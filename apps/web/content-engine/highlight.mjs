import hljs from 'highlight.js';
hljs.safeMode();
export function highlight(code, language) {
  return hljs.getLanguage(language) ? hljs.highlight(code, { language, ignoreIllegals: true }).value : null;
}
