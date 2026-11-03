// The world closing: the relay closes its UDP port.
fn voicechat_server_lifecycle(phase: crate::game::native_lifecycle::WorldPhase) {
    use crate::game::native_lifecycle::WorldPhase;
    if matches!(
        phase,
        WorldPhase::WorldCloseFinal | WorldPhase::WorldCloseFailed
    ) {
        crate::game::voicechat_server::stop();
    }
}
