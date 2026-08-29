use crate::freerdp::capabilities::FreeRdpCapabilities;
use crate::model::{
    ConnectionFailure, ConnectionPlan, FreeRdpClient, PlannedRoute, Profile, ResolvedCredentials,
    Route,
};
/// Create an IO-free plan from an already validated profile and selected client.
///
/// # Errors
///
/// Returns a typed failure for invalid configuration or unsupported capabilities.
pub fn plan(
    profile: &Profile,
    caps: &FreeRdpCapabilities,
    client: FreeRdpClient,
) -> Result<ConnectionPlan, ConnectionFailure> {
    if !profile.validate().is_empty() {
        return Err(ConnectionFailure::Configuration);
    }
    if (matches!(profile.display.renderer, crate::model::Renderer::WaylandSdl) && !caps.sdl)
        || (matches!(profile.display.renderer, crate::model::Renderer::X11) && !caps.x11)
    {
        return Err(ConnectionFailure::Renderer);
    }
    let (route, gateway) = match &profile.route {
        Route::Direct => (PlannedRoute::Direct, None),
        Route::RdGateway {
            gateway,
            credential,
        } => {
            if !caps.gateway {
                return Err(ConnectionFailure::UnsupportedCapability);
            }
            (
                PlannedRoute::RdGateway {
                    gateway: gateway.clone(),
                },
                *credential,
            )
        }
        Route::SshTunnel { jump_host } => (
            PlannedRoute::SshTunnel {
                jump_host: jump_host.clone(),
                target: profile.endpoint.clone(),
            },
            None,
        ),
    };
    Ok(ConnectionPlan {
        target: profile.endpoint.clone(),
        route,
        identity: profile.identity.clone(),
        display: profile.display.clone(),
        devices: profile.devices.clone(),
        security: profile.security.clone(),
        credentials: ResolvedCredentials {
            main: profile.credential,
            gateway,
        },
        client,
    })
}
