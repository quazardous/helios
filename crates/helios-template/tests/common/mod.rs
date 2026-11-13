//! What the tests share: one list of the modules the built-in templates
//! name, so a module a template starts to use is added in one place and
//! every test that loads the shipped templates sees it.

use helios_core::ModuleRef;
use helios_template::Catalogue;

/// The modules the built-in templates name: the raw chain, the tools on
/// a picture and the masks. Composites are added by each test.
pub fn modules() -> Catalogue {
    use helios_module_masks as m;
    Catalogue::new()
        .module(ModuleRef::new(helios_module_rawprepare::RawPrepare))
        .module(ModuleRef::new(helios_module_whitebalance::WhiteBalance))
        .module(ModuleRef::new(helios_module_highlights::Highlights))
        .module(ModuleRef::new(helios_module_demosaic::Demosaic))
        .module(ModuleRef::new(helios_module_colorin::ColorIn))
        .module(ModuleRef::new(helios_module_chartprofile::ChartProfile))
        .module(ModuleRef::new(helios_module_cameracolour::CameraColour))
        .module(ModuleRef::new(
            helios_module_channelmixerrgb::ChannelMixerRgb,
        ))
        .module(ModuleRef::new(helios_module_flip::Flip))
        .module(ModuleRef::new(helios_module_exposure::Exposure))
        .module(ModuleRef::new(helios_module_cameratone::CameraTone))
        .module(ModuleRef::new(helios_module_sigmoid::Sigmoid))
        .module(ModuleRef::new(m::MaskCombine))
        .module(ModuleRef::new(m::MaskFeather))
        .module(ModuleRef::new(m::MaskBlend))
        .module(ModuleRef::new(m::MaskObject))
}
