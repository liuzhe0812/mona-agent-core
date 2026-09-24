// Executes only in an opaque-origin, script-enabled sandbox. No token, model API, filesystem URL or parent DOM access.
(() => {
  const nonce = location.hash.slice(1); let started = false;
  document.addEventListener('click', event => { if (event.target.closest?.('a')) { event.preventDefault(); event.stopPropagation(); } }, true);
  const reply = (type, value = '') => parent.postMessage({ type, nonce, value }, '*');
  window.addEventListener('message', async event => {
    const message = event.data;
    if (event.source !== parent || message?.nonce !== nonce || message.type !== 'document' || started) return;
    started = true;
    try {
      if (!['docx', 'pptx', 'xlsx'].includes(message.kind) || !(message.buffer instanceof ArrayBuffer) || message.buffer.byteLength > 8 * 1024 * 1024) throw new Error('文档数据超限或格式不支持。');
      await new Promise((resolve, reject) => { const script = document.createElement('script'); script.src = `/apps/web/vendor/office/${message.kind}.js`; script.onload = resolve; script.onerror = () => reject(new Error('本地文档预览组件加载失败。')); document.head.append(script); });
      const cleanup = await window.MonaOffice.render(message.buffer, document.querySelector('#document'), message);
      window.addEventListener('pagehide', () => cleanup?.(), { once: true }); reply('rendered');
    } catch (error) { document.querySelector('#document').textContent = error.message; reply('error', error.message); }
  });
  reply('ready');
})();
