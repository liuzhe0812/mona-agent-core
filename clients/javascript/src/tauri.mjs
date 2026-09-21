import {BridgeError, isTerminal, frameSequence} from './index.mjs';

/** Dependency injection keeps the Web build free of @tauri-apps/api. */
export class TauriAgentClient {
  #invoke; #Channel; #idleTimeout;
  constructor({invoke, Channel, streamIdleTimeoutMs = 120000}) {
    if (typeof invoke !== 'function' || typeof Channel !== 'function') throw new Error('Tauri invoke and Channel are required');
    if (!Number.isFinite(streamIdleTimeoutMs) || streamIdleTimeoutMs <= 0) throw new Error('Positive streamIdleTimeoutMs required');
    this.#invoke = invoke; this.#Channel = Channel; this.#idleTimeout = streamIdleTimeoutMs;
  }
  #call(method, args) { return this.#invoke(`plugin:agent-bridge|${method}`, args); }
  start(request) { return this.#call('start_task', {request}); }
  cancel(runId) { return this.#call('cancel_task', {runId}); }
  input(runId, request) { return this.#call('send_input', {runId, request}); }
  snapshot(runId) { return this.#call('get_snapshot', {runId}); }
  result(runId) { return this.#call('get_result', {runId}); }
  forget(runId) { return this.#call('forget_run', {runId}); }
  subscribe(runId, {after, onFrame, signal} = {}) {
    if (typeof onFrame !== 'function') throw new Error('onFrame callback is required');
    if (after != null && (!Number.isSafeInteger(after) || after < 0)) throw new Error('Invalid cursor');
    let id, stopped = false, settled = false, resolve, reject, timer, cursor = after;
    const closed = new Promise((res, rej) => {resolve = res; reject = rej;});
    const detach = async () => { if (id) await this.#call('unsubscribe_events', {subscriptionId:id}); };
    const finish = (error) => {
      if (settled) return; settled = true; stopped = true; clearTimeout(timer);
      signal?.removeEventListener('abort', close);
      // Detach first; failure to detach cannot turn a task into cancelled.
      void detach().catch(() => {});
      if (error) {
        if (!(error instanceof BridgeError)) error = new BridgeError(error.message || 'Tauri stream failed', {code:error.code, cursor});
        error.cursor ??= cursor; reject(error);
      } else resolve();
    };
    const close = () => finish();
    const touch = () => {
      clearTimeout(timer);
      timer = setTimeout(() => finish(new BridgeError('Channel idle timeout; reconnect with the last cursor', {code:'disconnected', cursor})), this.#idleTimeout);
    };
    if (signal?.aborted) { finish(); return {closed, close}; }
    signal?.addEventListener('abort', close, {once:true});
    touch();
    const channel = new this.#Channel();
    channel.onmessage = packet => {
      id ??= packet.subscription_id; // first packet may precede the subscribe response
      if (packet.subscription_id !== id) { finish(new BridgeError('Mismatched channel subscription', {code:'protocol', cursor})); return; }
      if (!stopped) touch();
      void (async () => {
        if (stopped) { await detach(); return; }
        await onFrame(packet.frame);
        cursor = frameSequence(packet.frame) ?? cursor;
        if (stopped) return;
        await this.#call('ack_event', {subscriptionId:packet.subscription_id, deliveryId:packet.delivery_id});
        if (packet.frame.kind === 'fault') {
          throw new BridgeError(packet.frame.error.message, {code:packet.frame.error.code});
        }
        if (isTerminal(packet.frame)) finish();
      })().catch(finish);
    };
    void this.#call('subscribe_events', {runId, after:after ?? null, onEvent:channel})
      .then(async result => {
        if (id != null && id !== result.subscription_id) throw new BridgeError('Mismatched subscribe response', {code:'protocol', cursor});
        id = result.subscription_id;
        if (stopped) { await detach(); return; }
        // If the subscription starts exactly at a completed cursor, it may emit no packet.
        const snapshot = await this.snapshot(runId);
        if (snapshot.outcome != null && after != null && after >= snapshot.seq) finish();
      }).catch(finish);
    return {closed, close};
  }
}
