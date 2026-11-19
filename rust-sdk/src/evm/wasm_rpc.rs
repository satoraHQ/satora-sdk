use super::RPC_TIMEOUT;
use alloy::rpc::json_rpc::RequestPacket;
use alloy::rpc::json_rpc::ResponsePacket;
use alloy::transports::TransportError;
use alloy::transports::TransportErrorKind;
use alloy::transports::TransportFut;
use alloy::transports::http::ReqwestTransport;
use std::task::Context;
use std::task::Poll;
use tower_service::Service;

/// Reqwest's Fetch client has no builder-level timeout. Bound the whole RPC
/// request, including the response body, using a JS host timer.
#[derive(Clone)]
pub(super) struct TimedHttp(pub ReqwestTransport);

impl Service<RequestPacket> for TimedHttp {
    type Response = ResponsePacket;
    type Error = TransportError;
    type Future = TransportFut<'static>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, request: RequestPacket) -> Self::Future {
        let response = self.0.call(request);
        Box::pin(async move {
            crate::runtime::timeout(RPC_TIMEOUT, response)
                .await
                .map_err(|_| TransportErrorKind::custom_str("EVM RPC request timed out"))?
        })
    }
}
