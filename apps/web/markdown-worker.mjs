import { parseMarkdown } from './vendor/content/parser.mjs';
self.onmessage = ({ data }) => {
  try { self.postMessage({ id: data.id, tokens: parseMarkdown(data.source) }); }
  catch (error) { self.postMessage({ id: data.id, error: String(error.message || error).slice(0, 300) }); }
};
