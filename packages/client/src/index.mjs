export const PROTOCOL_VERSION = 2;
export const UI_TEXT_BYTES = 65536;
export const UI_RETAINED_ITEMS = 256;
const encoder = new TextEncoder();
const decoder = new TextDecoder('utf-8', { fatal: true });
export class BridgeError extends Error {
  constructor(message, { code = 'transport', status, cursor } = {}) {
    super(message); this.name = 'BridgeError'; this.code = code; this.status = status; this.cursor = cursor;
  }
}
export function requestId() { return globalThis.crypto.randomUUID(); }
export function isTerminal(frame) {
  return frame.kind === 'fault' || (frame.kind === 'snapshot' && frame.snapshot.outcome != null)
    || (frame.kind === 'event' && frame.envelope.event.type === 'run/completed');
}
export function frameSequence(frame) {
  return frame.kind === 'event' ? frame.envelope.seq : frame.kind === 'snapshot' ? frame.snapshot.seq : undefined;
}
function appendPreview(target, key, flag, text) {
  if (target[flag]) return;
  const room = UI_TEXT_BYTES - encoder.encode(target[key]).length;
  const bytes = encoder.encode(text);
  if (bytes.length <= room) { target[key] += text; return; }
  let end = Math.max(0, room);
  while (end > 0 && end < bytes.length && (bytes[end] & 0xc0) === 0x80) end--;
  target[key] += decoder.decode(bytes.subarray(0, end)); target[flag] = true;
}
function running(state) { return state === 'pending' || state === 'running'; }

/** Shared renderer state for either transport. Render strings as text, not innerHTML. */
export class RunView {
  constructor(runId) {
    this.state = {protocol_version: 2, run_id: runId, seq: 0, started: false, step: 0,
      items: [], pruned_items: 0, outcome: null};
  }
  apply(frame) {
    if (frame.kind === 'fault') throw new BridgeError(frame.error.message, {code: frame.error.code, cursor: this.state.seq});
    const incoming = frame.kind === 'snapshot' ? frame.snapshot : frame.envelope;
    if (!incoming || incoming.protocol_version !== 2 || incoming.run_id !== this.state.run_id
        || !Number.isSafeInteger(incoming.seq) || incoming.seq < 0) {
      throw new BridgeError('Invalid stream version, run identity or sequence', {code:'protocol'});
    }
    if (frame.kind === 'snapshot') {
      if (incoming.seq < this.state.seq) return false;
      this.state = structuredClone(incoming); return true;
    }
    if (incoming.seq <= this.state.seq) return false; // reconnect duplicate
    if (incoming.seq !== this.state.seq + 1) {
      throw new BridgeError('Stream gap: obtain a snapshot before applying more deltas', {code:'gap', cursor:this.state.seq});
    }
    const e = incoming.event;
    let item = e.item_id ? this.state.items.find(i => i.id === e.item_id) : null;
    switch (e.type) {
      case 'run/started': this.state.started = true; break;
      case 'step/started': this.state.step = e.step; break;
      case 'item/started': case 'item/updated': case 'item/completed': {
        const index = this.state.items.findIndex(i => i.id === e.item.id);
        if (index >= 0) this.state.items[index] = structuredClone(e.item);
        else {
          if (this.state.items.length >= UI_RETAINED_ITEMS) {
            let remove = this.state.items.findIndex(i => !running(i.state));
            this.state.items.splice(Math.max(0, remove), 1); this.state.pruned_items++;
          }
          this.state.items.push(structuredClone(e.item));
        }
        break;
      }
      case 'item/agentMessage/delta':
        if (item?.content.kind === 'agent_message') appendPreview(item.content, 'text', 'truncated', e.text);
        break;
      case 'item/toolCall/argumentsDelta':
        if (item?.content.kind === 'tool_call') {
          if (e.call_id != null) item.content.call_id = e.call_id;
          if (e.name != null) item.content.name = e.name;
          appendPreview(item.content, 'arguments_text', 'arguments_truncated', e.delta);
        }
        break;
      case 'item/toolCall/outputDelta':
        if (item?.content.kind === 'tool_call') appendPreview(item.content, 'output', 'output_truncated', e.text);
        break;
      case 'run/completed': this.state.outcome = structuredClone(e.outcome); break;
      case 'input/applied': case 'step/completed': break;
      default: throw new BridgeError(`Unknown protocol v2 event: ${e.type}`, {code:'protocol', cursor:this.state.seq});
    }
    this.state.seq = incoming.seq;
    return true;
  }
}
