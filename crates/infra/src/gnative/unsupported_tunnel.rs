use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_gnative_protocol::gnative::{
    input::GNativeInputEvent, session::GNativeSessionDescriptor,
};
use germinal_ports::service::gnative_tunnel::{GNativeTunnelError, IGNativeTunnel};
use std::marker::PhantomData;

pub struct GNativeTunnel<Dispatch>(PhantomData<Dispatch>);

impl<Dispatch> GNativeTunnel<Dispatch> {
    pub fn new() -> Result<Self, GNativeTunnelError> {
        Ok(Self(PhantomData))
    }
}

impl<Dispatch> IGNativeTunnel for GNativeTunnel<Dispatch> {
    fn ensure_session_descriptor(
        &self,
        _gshell_id: GShellId,
        _protocol_version: u32,
    ) -> Result<GNativeSessionDescriptor, GNativeTunnelError> {
        Err(GNativeTunnelError::UnsupportedPlatform)
    }
    fn begin_accept_session(&self, _gshell_id: GShellId) -> Result<(), GNativeTunnelError> {
        Err(GNativeTunnelError::UnsupportedPlatform)
    }
    fn send_input(
        &self,
        _gshell_id: GShellId,
        _input: GNativeInputEvent,
    ) -> Result<(), GNativeTunnelError> {
        Err(GNativeTunnelError::UnsupportedPlatform)
    }
    fn close_session(&self, _gshell_id: GShellId) -> Result<(), GNativeTunnelError> {
        Ok(())
    }
}
