// Pure presentation shared by the Shell control and its hardware-free tests.
export function describeSession(enabled, json, pending = null) {
    if (pending === 'on')
        return {checked: true, subtitle: 'Starting…', detail: 'Starting the tablet display'};
    if (pending === 'off')
        return {checked: false, subtitle: 'Stopping…', detail: 'Stopping the tablet display'};
    if (!enabled)
        return {checked: false, subtitle: 'Off', detail: 'Display is off'};

    let state;
    try {
        state = JSON.parse(json ?? '"Idle"');
    } catch {
        state = 'Idle';
    }
    if (state?.Streaming) {
        const s = state.Streaming;
        return {checked: true, subtitle: s.connection,
            detail: `Connected to ${s.device} over ${s.connection} — ${s.width} × ${s.height}`};
    }
    if (state?.Unauthorized)
        return {checked: true, subtitle: 'Allow USB', detail: 'Unlock the tablet and allow USB debugging'};
    if (state?.Failed)
        return {checked: true, subtitle: 'Needs attention', detail: 'Open Settings to check the connection'};
    if (state?.Connecting)
        return {checked: true, subtitle: 'Connecting…', detail: state.Connecting.step};
    return {checked: true, subtitle: 'Waiting', detail: 'Open Extraspace on your paired tablet, or connect USB'};
}
