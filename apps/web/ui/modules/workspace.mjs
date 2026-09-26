import { WorkspaceUI } from '../../workspace-ui.mjs';
import { claim, scoped, setting } from '../dom.mjs';
export const version = 1;
export function mount(ctx) {
  const roots = ['workspace-settings-section','projects-region','workspace-files-open','projects-notice','project-dialog'].map(claim);
  roots.push(document.getElementById('project-picker'), document.getElementById('composer-project-menu'));
  roots.push(...['header-workspace-context','header-workspace-popover','header-open-group','header-open-menu'].map(claim));
  const ui = new WorkspaceUI({
    query: scoped(roots), pane: ctx.shell.pane(ctx.id),
    ready: () => ctx.emit('workspace-ready', ui),
    currentProject: ctx.shell.project, selectProject: ctx.shell.selectProject,
    pickDesktopProject: ctx.shell.pickDesktopProject,
    desktopMode: ctx.shell.desktopMode,
    desktopOpenWith: ctx.shell.desktopOpenWith,
    openDesktopWorkspace: ctx.shell.openDesktopWorkspace,
    projectsChanged: () => ctx.shell.projectsChanged(ui.projectState),
    projectsRendered: projects => ctx.shell.projectsRendered(projects),
    showMoreProject: id => ctx.shell.showMoreProject(id),
  });
  ctx.own(() => ui.dispose()); ctx.provide('workspace', ui);
  setting(ctx, 'workspace', roots[0], () => ui.refreshSettings(), 50);
  ctx.register('presentation', { id: 'workspace', value: session => ui.presentation(session) });
  ctx.on('session', session => ui.setSession(session));
  ctx.on('connection', async connection => { if (connection) await ui.configure(connection.base, connection.bearer); else ui.clear(); });
}
