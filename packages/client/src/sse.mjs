/** Strict, bounded SSE reader; handles split UTF-8/CRLF, comments and multiline data. */
export async function* readSse(body, maxEventBytes = 128 * 1024 * 1024) {
  if (!Number.isSafeInteger(maxEventBytes) || maxEventBytes <= 0) throw new Error('Invalid SSE byte limit');
  if (!body) throw new Error('Missing streaming response body');
  const reader = body.getReader();
  const decoder = new TextDecoder('utf-8', {fatal:true});
  const encoder = new TextEncoder();
  let buffer = '', data = [], event = 'message', id, eventBytes = 0;
  try {
    while (true) {
      const {done, value} = await reader.read();
      buffer += done ? decoder.decode() : decoder.decode(value, {stream:true});
      while (true) {
        const match = /\r\n|\r|\n/.exec(buffer);
        if (!match) break;
        if (!done && match[0] === '\r' && match.index === buffer.length - 1) break;
        const line = buffer.slice(0, match.index);
        buffer = buffer.slice(match.index + match[0].length);
        if (line === '') {
          if (data.length) yield {event, id, data:data.join('\n')};
          data = []; event = 'message'; eventBytes = 0;
          continue;
        }
        eventBytes += encoder.encode(line).length + 1;
        if (eventBytes > maxEventBytes) throw new Error('SSE frame exceeds configured byte limit');
        if (line.startsWith(':')) continue;
        const colon = line.indexOf(':');
        const field = colon < 0 ? line : line.slice(0, colon);
        let text = colon < 0 ? '' : line.slice(colon + 1);
        if (text.startsWith(' ')) text = text.slice(1);
        if (field === 'data') { data.push(text); }
        else if (field === 'event') event = text;
        else if (field === 'id' && !text.includes('\0')) id = text;
      }
      if (encoder.encode(buffer).length + eventBytes > maxEventBytes) throw new Error('SSE frame exceeds configured byte limit');
      if (done) {
        if (buffer.trim() || data.length) throw new Error('SSE stream ended in an incomplete frame');
        break;
      }
    }
  } finally {
    try { await reader.cancel(); } catch { /* transport may already be closed */ }
    reader.releaseLock();
  }
}
