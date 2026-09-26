import mermaid from 'mermaid';
const nonce = location.hash.slice(1); let busy = false;
const reply = value => parent.postMessage({ nonce, ...value }, '*');
window.addEventListener('message', async event => {
  const data = event.data;
  if (event.source !== parent || data?.nonce !== nonce || data.type !== 'render' || busy) return;
  busy = true;
  try {
    if (typeof data.source !== 'string' || data.source.length > 32768) throw new Error('图表超过 32 KiB 渲染上限。');
    mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', suppressErrorRendering: true, maxTextSize: 32768, maxEdges: 500,
      theme: data.dark ? 'dark' : 'default', fontFamily: 'system-ui, sans-serif', flowchart: { htmlLabels: false },
      secure: ['secure', 'securityLevel', 'startOnLoad', 'maxTextSize', 'maxEdges', 'suppressErrorRendering', 'fontFamily', 'theme', 'themeCSS', 'themeVariables', 'dompurifyConfig'] });
    const { svg } = await mermaid.render('mona_diagram_' + data.id, data.source);
    if (svg.length > 2 * 1024 * 1024) throw new Error('图表输出超过预览上限。');
    reply({ type: 'result', id: data.id, svg });
  } catch (error) { reply({ type: 'error', id: data.id, message: String(error?.message || error).slice(0, 500) }); }
  finally { busy = false; }
});
reply({ type: 'ready' });
