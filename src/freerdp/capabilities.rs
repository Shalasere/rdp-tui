use semver::Version;
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AuthOnlySupport {
    Validated,
    NotSupported,
    Unvalidated,
}
#[derive(Debug, Clone, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // Binding shape is specified by 01-types.yaml.
pub struct FreeRdpCapabilities {
    pub version: Version,
    pub sdl: bool,
    pub x11: bool,
    pub askpass: bool,
    pub gateway: bool,
    pub multimon: bool,
    pub dynamic_resolution: bool,
    pub auth_only: AuthOnlySupport,
}
