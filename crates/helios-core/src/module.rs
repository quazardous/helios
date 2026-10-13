//! The contract a module implements.

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::error::Error;
use crate::image::{Image, Region};
use crate::manifest::{Manifest, ParamHint};
use crate::network::{Composite, Expansion, Network};
use crate::params::{Params, Value};

/// Context inputs: metadata of the image being processed (for the photo
/// domain, EXIF fields...). A module only sees the fields its manifest declares.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Context(BTreeMap<String, Value>);

impl Context {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, name: &str, value: Value) -> Self {
        self.0.insert(name.to_string(), value);
        self
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    /// Only the declared fields, so a module cannot read what it did not declare.
    pub(crate) fn restricted_to(&self, fields: &[String]) -> Context {
        Context(
            self.0
                .iter()
                .filter(|(k, _)| fields.contains(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        )
    }

    pub(crate) fn canonical_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.0).expect("context always serializes")
    }
}

/// A module's GPU variant: a WGSL compute kernel, run by a GPU backend
/// (`helios-gpu`) over the same plan as the CPU code, and checked against
/// it. The core only carries it: it never runs it.
///
/// One invocation per output pixel, workgroups of 64 along x. The
/// invocations form a two-dimensional grid (a GPU caps workgroups per
/// dimension, 65535 at least, which along one dimension would stop near
/// 4.2 Mpx): pixel `id.y * shape[0].w + id.x`, and an invocation past the
/// pixel count does nothing. Bindings, group 0: `0` the input
/// (`array<f32>`, read; it covers the region `input_region` asks for), `1`
/// the output (`array<f32>`, read and write), `2` the plan's uniforms
/// (`array<vec4<f32>>`), `3` the shape (`array<vec4<u32>, 2>`: output pixel
/// count, input channels, output channels, invocations per grid row; then
/// input width, input height, output width, output height). The output
/// region sits in the input at `(input width - output width) / 2` on each
/// side. A third `vec4` of the shape holds the output's origin and the
/// input's, `x` and `y` (`bitcast<i32>`): output x, output y, input x,
/// input y; a kernel that does not need them declares `array<vec4<u32>, 2>`
/// as before. A module without inputs (a generator, such as a drawn mask
/// shape) reads a one-pixel placeholder holding 0 at the output's origin,
/// so its kernel has the same bindings; it must still use binding 0
/// (`_ = input[0];`), since a pipeline's layout comes from its shader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuKernel {
    pub wgsl: &'static str,
    pub entry: &'static str,
}

/// Where a module's outputs depend on anything at all, and what they hold
/// everywhere else (a drawn shape is 0 away from it). The engine plans a
/// step outside `region` without its inputs and fills it with `fill`
/// instead of running the module, so `process` must give exactly `fill`
/// (bit for bit) on every channel of every output outside `region`: a
/// tiled render, some tiles filled, then equals one render.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extent {
    /// In pixels at the render's scale.
    pub region: Region,
    pub fill: f32,
}

/// What a GPU backend needs to run a module over a region.
#[derive(Clone, Debug, PartialEq)]
pub struct GpuJob {
    pub kernel: GpuKernel,
    pub uniforms: Vec<f32>,
    /// The input region the kernel reads.
    pub input: Region,
}

/// An image held in an accelerator's memory (a buffer on a GPU): the
/// region and channels it covers, and a handle only the accelerator that
/// made it understands. Cloning shares the handle; the memory is freed when
/// the last clone goes.
#[derive(Clone)]
pub struct DeviceImage {
    region: Region,
    channels: u32,
    handle: Arc<dyn Any + Send + Sync>,
}

impl DeviceImage {
    pub fn new(region: Region, channels: u32, handle: impl Any + Send + Sync) -> Self {
        Self {
            region,
            channels,
            handle: Arc::new(handle),
        }
    }

    pub fn region(&self) -> Region {
        self.region
    }

    pub fn channels(&self) -> u32 {
        self.channels
    }

    /// The accelerator's handle, if it is of type `T`.
    pub fn handle<T: Any>(&self) -> Option<&T> {
        self.handle.downcast_ref()
    }

    /// The bytes of its pixels (f32 per channel).
    pub fn bytes(&self) -> usize {
        self.region.area() * self.channels as usize * 4
    }
}

impl std::fmt::Debug for DeviceImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceImage")
            .field("region", &self.region)
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

/// What went wrong on an accelerator, as the engine needs to know it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    /// A kernel did not compile.
    Compile,
    /// A kernel did not run (a validation error, a wrong result shape).
    Dispatch,
    /// Its memory budget, or the device's memory, is spent.
    OutOfMemory,
    /// A wait for the device ran past its deadline: the device is hung.
    Timeout,
    /// The device is gone (a driver reset, a GPU unplugged).
    DeviceLost,
    /// Anything else.
    Other,
}

impl Failure {
    /// Whether the device itself is unusable: nothing it holds can be read
    /// back, so the engine rebuilds it and replays the render (or goes on
    /// without it), where other failures send one step to the CPU.
    pub fn is_device(self) -> bool {
        matches!(self, Failure::Timeout | Failure::DeviceLost)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Failure::Compile => "compile",
            Failure::Dispatch => "dispatch",
            Failure::OutOfMemory => "out of memory",
            Failure::Timeout => "timeout",
            Failure::DeviceLost => "device lost",
            Failure::Other => "other",
        }
    }
}

/// An accelerator's failure: what kind, and why in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceleratorError {
    pub failure: Failure,
    pub reason: String,
}

impl AcceleratorError {
    pub fn new(failure: Failure, reason: impl Into<String>) -> Self {
        Self {
            failure,
            reason: reason.into(),
        }
    }
}

impl From<String> for AcceleratorError {
    fn from(reason: String) -> Self {
        Self::new(Failure::Other, reason)
    }
}

impl From<&str> for AcceleratorError {
    fn from(reason: &str) -> Self {
        Self::new(Failure::Other, reason)
    }
}

impl std::fmt::Display for AcceleratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

/// An accelerator's memory: bytes in use and the budget they stay within.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Memory {
    pub used: u64,
    pub budget: u64,
}

/// Runs modules' GPU variants for the engine (`helios-gpu` on wgpu). The
/// engine hands it a step whose module has a kernel, one input and one
/// output. Images stay in its memory from one step to the next: the engine
/// uploads a host image only when a step it runs reads one, and reads a
/// result back only when the CPU (a CPU step, or the render's caller) needs
/// it.
///
/// A failure is typed ([`Failure`]): a step whose kernel fails is run on
/// the CPU instead, an allocation over budget first makes the engine evict
/// device content, and a lost or hung device is rebuilt ([`recover`]) and
/// the render replayed, or the engine goes on without the accelerator.
///
/// [`recover`]: Accelerator::recover
pub trait Accelerator: Send + Sync {
    /// Names the backend in content keys: its results agree with the CPU
    /// within a tolerance, not bit for bit, so they are cached apart.
    fn id(&self) -> &str;

    /// Copies a host image into its memory.
    fn upload(&self, image: &Image) -> Result<DeviceImage, AcceleratorError>;

    /// Reads one of its images back into host memory.
    fn download(&self, image: &DeviceImage) -> Result<Image, AcceleratorError>;

    /// Runs `job` over `output`, reading `input` (which covers
    /// `job.input`), into `channels` channels. Input and result stay in its
    /// memory.
    fn run(
        &self,
        module: &str,
        job: &GpuJob,
        input: &DeviceImage,
        output: Region,
        channels: u32,
    ) -> Result<DeviceImage, AcceleratorError>;

    /// Its memory in use and its budget, when it keeps one.
    fn memory(&self) -> Option<Memory> {
        None
    }

    /// Sets its memory budget in bytes; `None` goes back to its default.
    fn set_memory_budget(&self, _bytes: Option<u64>) {}

    /// Rebuilds its device after a [`Failure::is_device`] failure, so work
    /// can be replayed on it. An error when it cannot (it never could, the
    /// rebuild failed, or it was rebuilt too often): the engine then goes
    /// on without it.
    fn recover(&self) -> Result<(), String> {
        Err("this accelerator cannot rebuild its device".into())
    }
}

/// An image processing module: a pure computation with a declared surface.
///
/// No IO, no global state, no logging, no threads of its own: everything it
/// reads is an argument, everything it produces is returned. The same plan
/// and inputs always give the same outputs.
pub trait Module: Send + Sync + 'static {
    /// What `prepare` turns parameters and context into: the values `process` uses.
    type Plan: Send + Sync + 'static;

    fn manifest(&self) -> Manifest;

    /// How its parameters are best shown and moved (soft range, steps,
    /// unit): read by editors and agents, never by the engine. None by
    /// default.
    fn param_hints(&self) -> Vec<ParamHint> {
        Vec::new()
    }

    /// Turns parameters (already checked against the manifest and completed
    /// with defaults) and declared context fields into a plan. Cheap, no pixels.
    fn prepare(&self, params: &Params, context: &Context) -> Result<Self::Plan, String>;

    /// The plan for a render at `scale` (1 is full resolution, 0.5 half of
    /// it). Regions are then in pixels at that scale: a module whose
    /// parameters are sizes in pixels (a blur radius) turns them into pixels
    /// at the render's scale here, so that the result looks the same at any
    /// scale. The default ignores the scale: most modules work pixel by pixel.
    fn prepare_at(
        &self,
        params: &Params,
        context: &Context,
        scale: f64,
    ) -> Result<Self::Plan, String> {
        let _ = scale;
        self.prepare(params, context)
    }

    /// Where its outputs depend on its inputs and parameters (see
    /// [`Extent`]); `None`, the default, for everywhere.
    fn extent(&self, _plan: &Self::Plan) -> Option<Extent> {
        None
    }

    /// The region of each input needed to compute `output`. The default reads
    /// the output region grown by the declared overlap.
    fn input_region(&self, _plan: &Self::Plan, output: Region, overlap: u32) -> Region {
        output.grow(overlap)
    }

    /// Computes the outputs over `output`, one image per declared output port.
    /// `inputs` cover the regions asked for by `input_region`.
    fn process(
        &self,
        plan: &Self::Plan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String>;

    /// The GPU variant, if the module has one.
    fn gpu_kernel(&self) -> Option<GpuKernel> {
        None
    }

    /// The plan as the GPU kernel's uniforms, padded to whole `vec4`s.
    fn gpu_uniforms(&self, _plan: &Self::Plan) -> Vec<f32> {
        Vec::new()
    }
}

/// A module behind a uniform interface, as the graph holds it.
#[derive(Clone)]
pub struct ModuleRef(Arc<dyn Erased>);

impl ModuleRef {
    pub fn new<M: Module>(module: M) -> Self {
        let manifest = module.manifest();
        Self(Arc::new(Adapter { module, manifest }))
    }

    /// A node that stands for a network (a macro, a multi-kernel module):
    /// the engine expands it into its inner nodes when it plans a render.
    pub fn composite<C: Composite>(composite: C) -> Self {
        let manifest = composite.manifest();
        Self(Arc::new(CompositeAdapter {
            composite,
            manifest,
        }))
    }

    /// Whether it stands for a network.
    pub fn is_composite(&self) -> bool {
        self.0.is_composite()
    }

    /// The network a composite is with these parameters (checked and
    /// completed with defaults) at `at`, checked; `None` for a module that
    /// computes itself.
    pub fn expand(
        &self,
        params: &Params,
        context: &Context,
        at: &Expansion,
    ) -> Result<Option<Network>, Error> {
        if !self.is_composite() {
            return Ok(None);
        }
        let context = context.restricted_to(&self.manifest().context);
        let network = self
            .0
            .expand(params, &context, at)
            .map_err(|reason| self.module_error(reason))?;
        network.check(&self.manifest().name)?;
        if network.inputs().len() != self.manifest().inputs.len()
            || network.outputs().len() != self.manifest().outputs.len()
        {
            return Err(Error::Contract {
                module: self.manifest().name.clone(),
                reason: "its network's ports are not its own".into(),
            });
        }
        Ok(Some(network))
    }

    pub fn manifest(&self) -> &Manifest {
        self.0.manifest()
    }

    pub fn param_hints(&self) -> Vec<ParamHint> {
        self.0.param_hints()
    }

    pub(crate) fn prepare(
        &self,
        params: &Params,
        context: &Context,
        scale: f64,
    ) -> Result<Plan, Error> {
        let context = context.restricted_to(&self.manifest().context);
        self.0
            .prepare(params, &context, scale)
            .map_err(|reason| self.module_error(reason))
    }

    pub(crate) fn input_region(&self, plan: &Plan, output: Region) -> Region {
        self.0.input_region(plan, output)
    }

    pub(crate) fn extent(&self, plan: &Plan) -> Option<Extent> {
        self.0.extent(plan)
    }

    /// The region of its inputs it reads to compute `output` with these
    /// parameters at `scale` (its overlap, for a test that must really
    /// tile).
    pub fn reads(
        &self,
        params: &Params,
        context: &Context,
        scale: f64,
        output: Region,
    ) -> Result<Region, Error> {
        self.reads_in(params, context, &Expansion::new(scale, None), output)
    }

    /// [`ModuleRef::reads`] for an image of a known extent (`at`), which a
    /// composite whose sizes are fractions of the image needs. A
    /// composite reads the bounds of what its inner network reads of its
    /// inputs.
    pub fn reads_in(
        &self,
        params: &Params,
        context: &Context,
        at: &Expansion,
        output: Region,
    ) -> Result<Region, Error> {
        let params = self.manifest().resolve(params)?;
        if let Some(network) = self.expand(&params, context, at)? {
            let reads = network.reads(context, at, output)?;
            return Ok(reads
                .into_iter()
                .flatten()
                .reduce(|a, b| a.bounds(&b))
                .unwrap_or(output));
        }
        let plan = self.prepare(&params, context, at.scale)?;
        Ok(self.input_region(&plan, output))
    }

    /// What a GPU backend needs to run this module with these parameters:
    /// its kernel and the plan's uniforms. `None` when it has no GPU variant.
    pub fn gpu_job(
        &self,
        params: &Params,
        context: &Context,
        scale: f64,
        output: Region,
    ) -> Result<Option<GpuJob>, Error> {
        let Some(kernel) = self.0.gpu_kernel() else {
            return Ok(None);
        };
        let params = self.manifest().resolve(params)?;
        let plan = self.prepare(&params, context, scale)?;
        Ok(Some(GpuJob {
            kernel,
            uniforms: self.0.gpu_uniforms(&plan),
            input: self.gpu_input(&plan, output),
        }))
    }

    /// The region a GPU kernel reads: its input's, or for a module without
    /// inputs the placeholder's, one pixel at the output's origin.
    fn gpu_input(&self, plan: &Plan, output: Region) -> Region {
        if self.manifest().inputs.is_empty() {
            Self::placeholder(output).region()
        } else {
            self.input_region(plan, output)
        }
    }

    /// What a module without inputs reads on an accelerator for `output`:
    /// one pixel holding 0 at the output's origin.
    pub fn placeholder(output: Region) -> Image {
        Image::new(Region::new(output.x, output.y, 1, 1), 1, vec![0.0])
    }

    /// Whether an accelerator can run this module: a GPU variant, one
    /// output, and one input or none.
    pub(crate) fn acceleratable(&self) -> bool {
        let m = self.manifest();
        // An optional input may arrive empty: the CPU code handles it.
        self.0.gpu_kernel().is_some()
            && (m.inputs.is_empty() || (m.inputs.len() == 1 && !m.inputs[0].optional))
            && m.outputs.len() == 1
    }

    /// Runs the GPU variant on an accelerator and checks the result as
    /// `process` does; input and result stay in the accelerator's memory. A
    /// failure (a shader that does not compile, a lost device, a result of
    /// the wrong shape) is the accelerator's error, never a panic: the
    /// engine decides where the step runs instead.
    pub(crate) fn accelerate(
        &self,
        accelerator: &dyn Accelerator,
        plan: &Plan,
        input: &DeviceImage,
        output: Region,
    ) -> Result<DeviceImage, AcceleratorError> {
        let kernel = self.0.gpu_kernel().expect("checked by acceleratable");
        let job = GpuJob {
            kernel,
            uniforms: self.0.gpu_uniforms(plan),
            input: self.gpu_input(plan, output),
        };
        let m = self.manifest();
        let channels = m.outputs[0].channels().unwrap_or(input.channels());
        let image = accelerator.run(&m.name, &job, input, output, channels)?;
        self.check_shape(0, image.region(), image.channels(), output)
            .map_err(|e| AcceleratorError::new(Failure::Dispatch, e.to_string()))?;
        Ok(image)
    }

    /// Runs `process` and checks the result against the manifest.
    pub(crate) fn process(
        &self,
        plan: &Plan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, Error> {
        let outputs = self
            .0
            .process(plan, inputs, output)
            .map_err(|reason| self.module_error(reason))?;
        self.check(outputs, output)
    }

    /// Outputs as the manifest declares them: count, region, channels.
    fn check(&self, outputs: Vec<Image>, output: Region) -> Result<Vec<Image>, Error> {
        let m = self.manifest();
        let broken = |reason: String| Error::Contract {
            module: m.name.clone(),
            reason,
        };
        if outputs.len() != m.outputs.len() {
            return Err(broken(format!(
                "{} outputs, {} declared",
                outputs.len(),
                m.outputs.len()
            )));
        }
        for (i, image) in outputs.iter().enumerate() {
            self.check_shape(i, image.region(), image.channels(), output)?;
        }
        Ok(outputs)
    }

    /// Output `index` covers `output` with its declared channels.
    fn check_shape(
        &self,
        index: usize,
        region: Region,
        channels: u32,
        output: Region,
    ) -> Result<(), Error> {
        let m = self.manifest();
        let port = &m.outputs[index];
        let broken = |reason: String| Error::Contract {
            module: m.name.clone(),
            reason,
        };
        if region != output {
            return Err(broken(format!(
                "output `{}` covers {region}, asked {output}",
                port.name
            )));
        }
        if Some(channels) != port.channels() {
            return Err(broken(format!(
                "output `{}` has {channels} channels, {:?} declared",
                port.name,
                port.channels()
            )));
        }
        Ok(())
    }

    fn module_error(&self, reason: String) -> Error {
        Error::Module {
            module: self.manifest().name.clone(),
            reason,
        }
    }
}

/// A prepared plan, type-erased.
pub(crate) type Plan = Arc<dyn Any + Send + Sync>;

trait Erased: Send + Sync {
    fn manifest(&self) -> &Manifest;
    fn param_hints(&self) -> Vec<ParamHint>;
    fn prepare(&self, params: &Params, context: &Context, scale: f64) -> Result<Plan, String>;
    fn input_region(&self, plan: &Plan, output: Region) -> Region;
    fn extent(&self, plan: &Plan) -> Option<Extent>;
    fn gpu_kernel(&self) -> Option<GpuKernel>;
    fn gpu_uniforms(&self, plan: &Plan) -> Vec<f32>;
    fn process(&self, plan: &Plan, inputs: &[&Image], output: Region)
    -> Result<Vec<Image>, String>;
    fn is_composite(&self) -> bool {
        false
    }
    fn expand(&self, _: &Params, _: &Context, _: &Expansion) -> Result<Network, String> {
        Err("not a composite".into())
    }
}

/// A composite behind the uniform interface: it never prepares or
/// processes, the engine expands it.
struct CompositeAdapter<C: Composite> {
    composite: C,
    manifest: Manifest,
}

impl<C: Composite> Erased for CompositeAdapter<C> {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn param_hints(&self) -> Vec<ParamHint> {
        self.composite.param_hints()
    }

    fn prepare(&self, _: &Params, _: &Context, _: f64) -> Result<Plan, String> {
        Ok(Arc::new(()))
    }

    fn input_region(&self, _: &Plan, output: Region) -> Region {
        output
    }

    fn extent(&self, _: &Plan) -> Option<Extent> {
        None
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        None
    }

    fn gpu_uniforms(&self, _: &Plan) -> Vec<f32> {
        Vec::new()
    }

    fn process(&self, _: &Plan, _: &[&Image], _: Region) -> Result<Vec<Image>, String> {
        Err("a composite is expanded into its network, never processed".into())
    }

    fn is_composite(&self) -> bool {
        true
    }

    fn expand(
        &self,
        params: &Params,
        context: &Context,
        at: &Expansion,
    ) -> Result<Network, String> {
        self.composite.expand(params, context, at)
    }
}

struct Adapter<M: Module> {
    module: M,
    manifest: Manifest,
}

impl<M: Module> Adapter<M> {
    fn plan(plan: &Plan) -> &M::Plan {
        plan.downcast_ref()
            .expect("a plan is only ever given back to the module that made it")
    }
}

impl<M: Module> Erased for Adapter<M> {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn param_hints(&self) -> Vec<ParamHint> {
        self.module.param_hints()
    }

    fn prepare(&self, params: &Params, context: &Context, scale: f64) -> Result<Plan, String> {
        Ok(Arc::new(self.module.prepare_at(params, context, scale)?))
    }

    fn input_region(&self, plan: &Plan, output: Region) -> Region {
        self.module
            .input_region(Self::plan(plan), output, self.manifest.needs.overlap)
    }

    fn extent(&self, plan: &Plan) -> Option<Extent> {
        self.module.extent(Self::plan(plan))
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        self.module.gpu_kernel()
    }

    fn gpu_uniforms(&self, plan: &Plan) -> Vec<f32> {
        let mut u = self.module.gpu_uniforms(Self::plan(plan));
        u.resize(u.len().div_ceil(4).max(1) * 4, 0.0);
        u
    }

    fn process(
        &self,
        plan: &Plan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        self.module.process(Self::plan(plan), inputs, output)
    }
}
