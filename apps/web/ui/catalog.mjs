// Explicit build-time product catalog. Server IDs select these functions, never arbitrary URLs.
export const catalog = Object.freeze({
  mcp: { load: () => import('./modules/mcp.mjs') },
  appearance: { local: true, load: () => import('./modules/appearance.mjs') },
  details: { local: true, load: () => import('./modules/details.mjs') },
  'conversation-rail': { local: true, load: () => import('./modules/conversation-rail.mjs') },
  capabilities: { load: () => import('./modules/capabilities.mjs') },
  models: { load: () => import('./modules/models.mjs') },
  memory: { load: () => import('./modules/memory.mjs') },
  workspace: { load: () => import('./modules/workspace.mjs') },
  workbench: { requires: ['workspace'], load: () => import('./modules/workbench.mjs') },
  side: { requires: ['workspace'], load: () => import('./modules/side.mjs') },
  metrics: { requires: ['workspace'], load: () => import('./modules/metrics.mjs') },
  planner: { requires: ['workspace'], load: () => import('./modules/planner.mjs') },
  sandbox: { requires: ['workspace'], load: () => import('./modules/sandbox.mjs') },
  subagent: { requires: ['workspace'], load: () => import('./modules/subagent.mjs') },
});
