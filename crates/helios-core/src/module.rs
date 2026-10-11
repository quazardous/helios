//! The contract a module implements.

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::error::Error;
use crate::image::{Image, Region};
use crate::manifest::Manifest;
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
/// One invocation per output pixel, workgroups of 64. Bindings, group 0:
/// `0` the input (`array<f32>`, read; it covers the region `input_region`
/// asks for), `1` the output (`array<f32>`, read and write), `2` the plan's
/// uniforms (`array<vec4<f32>>`), `3` the shape (`array<vec4<u32>, 2>`:
/// output pixel count, input channels, output channels; then input width,
/// input height, output width, output height). The output region sits in
/// the input at `(input width - output width) / 2` on each side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuKernel {
    pub wgsl: &'static str,
    pub entry: &'static str,
}

/// What a GPU backend needs to run a module over a region.
#[derive(Clone, Debug, PartialEq)]
pub struct GpuJob {
    pub kernel: GpuKernel,
    pub uniforms: Vec<f32>,
    /// The input region the kernel reads.
    pub input: Region,
}

/// Runs modules' GPU variants for the engine (`helios-gpu` on wgpu). The
/// engine hands it a step whose module has a kernel, one input and one
/// output; it answers the output image, or why it could not.
pub trait Accelerator: Send + Sync {
    /// Names the backend in content keys: its results agree with the CPU
    /// within a tolerance, not bit for bit, so they are cached apart.
    fn id(&self) -> &str;

    /// Runs `job` over `output`, reading `input` (which covers
    /// `job.input`), into `channels` channels.
    fn run(
        &self,
        module: &str,
        job: &GpuJob,
        input: &Image,
        output: Region,
        channels: u32,
    ) -> Result<Image, String>;
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

    pub fn manifest(&self) -> &Manifest {
        self.0.manifest()
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
            input: self.input_region(&plan, output),
        }))
    }

    /// Whether an accelerator can run this module: a GPU variant, one
    /// input and one output.
    pub(crate) fn acceleratable(&self) -> bool {
        let m = self.manifest();
        self.0.gpu_kernel().is_some() && m.inputs.len() == 1 && m.outputs.len() == 1
    }

    /// Runs the GPU variant on an accelerator and checks the result as
    /// `process` does. A failure (a shader that does not compile, a lost
    /// device) is the module's error, never a panic.
    pub(crate) fn accelerate(
        &self,
        accelerator: &dyn Accelerator,
        plan: &Plan,
        input: &Image,
        output: Region,
    ) -> Result<Vec<Image>, Error> {
        let kernel = self.0.gpu_kernel().expect("checked by acceleratable");
        let job = GpuJob {
            kernel,
            uniforms: self.0.gpu_uniforms(plan),
            input: self.input_region(plan, output),
        };
        let m = self.manifest();
        let channels = m.outputs[0].channels().unwrap_or(input.channels());
        let image = accelerator
            .run(&m.name, &job, input, output, channels)
            .map_err(|reason| self.module_error(format!("GPU: {reason}")))?;
        self.check(vec![image], output)
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
        for (image, port) in outputs.iter().zip(&m.outputs) {
            if image.region() != output {
                return Err(broken(format!(
                    "output `{}` covers {}, asked {output}",
                    port.name,
                    image.region()
                )));
            }
            if Some(image.channels()) != port.channels() {
                return Err(broken(format!(
                    "output `{}` has {} channels, {:?} declared",
                    port.name,
                    image.channels(),
                    port.channels()
                )));
            }
        }
        Ok(outputs)
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
    fn prepare(&self, params: &Params, context: &Context, scale: f64) -> Result<Plan, String>;
    fn input_region(&self, plan: &Plan, output: Region) -> Region;
    fn gpu_kernel(&self) -> Option<GpuKernel>;
    fn gpu_uniforms(&self, plan: &Plan) -> Vec<f32>;
    fn process(&self, plan: &Plan, inputs: &[&Image], output: Region)
    -> Result<Vec<Image>, String>;
}

struct Adapter<M: Module> {
    module: M,
    manifest: Manifest,
}

impl<M: Module> Adapter<M> {
    fn plan<'a>(&self, plan: &'a Plan) -> &'a M::Plan {
        plan.downcast_ref()
            .expect("a plan is only ever given back to the module that made it")
    }
}

impl<M: Module> Erased for Adapter<M> {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn prepare(&self, params: &Params, context: &Context, scale: f64) -> Result<Plan, String> {
        Ok(Arc::new(self.module.prepare_at(params, context, scale)?))
    }

    fn input_region(&self, plan: &Plan, output: Region) -> Region {
        self.module
            .input_region(self.plan(plan), output, self.manifest.needs.overlap)
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        self.module.gpu_kernel()
    }

    fn gpu_uniforms(&self, plan: &Plan) -> Vec<f32> {
        let mut u = self.module.gpu_uniforms(self.plan(plan));
        u.resize(u.len().div_ceil(4).max(1) * 4, 0.0);
        u
    }

    fn process(
        &self,
        plan: &Plan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        self.module.process(self.plan(plan), inputs, output)
    }
}
