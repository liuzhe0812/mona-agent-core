import type { UiOptions, WorkspaceState, Conversation, Theme } from './types.js';
import { WorkspaceController } from './controller.js';
import { el, button, textButton, saveFile } from './dom.js';
import { icon } from './icons.js';
import { dialog, nameDialog } from './components/dialog.js';
import { Composer } from './components/composer.js';
import { TurnCard, outcomeLabel } from './components/items.js';
/** Custom element boundary isolates CSS and injects, rather than owns, the runtime client. */
export class MonaAgentElement extends HTMLElement {
    private options?: UiOptions;
    private ctrl?: WorkspaceController;
    private unsubscribe?: () => void;
    private root = this.attachShadow({ mode: 'open' });
    private shell = el('div');
    private sidebar = el('aside');
    private groups = el('div');
    private thread = el('main');
    private welcome = el('section');
    private timeline = el('div');
    private heading = el('span');
    private breadcrumb = el('span');
    private inspector = el('aside');
    private composer?: Composer;
    private toast = el('div');
    private cards = new Map<string, TurnCard>();
    private lastConversationId: string | null = null;
    private sidebarKey = '';
    private state?: WorkspaceState;
    private scrollButton = el('button');
    private titleActions = el('div');
    private theme: Theme = 'light';
    private themeQuery = matchMedia('(prefers-color-scheme: dark)');
    private toastTimer?: ReturnType<typeof setTimeout>;
    private themeListener = () => this.applyTheme();
    private sizeQuery = matchMedia('(max-width: 760px)');
    private sizeListener = () => this.syncSidebarAccessibility();
    private frame?: number;
    private onKeyboard = (e: KeyboardEvent) => {
        if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
            e.preventDefault();
            this.search();
        }
        if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'n') {
            e.preventDefault();
            this.newConversation();
        }
        if (e.key === 'Escape') {
            this.shell.classList.remove('mobile-sidebar');
            this.syncSidebarAccessibility();
        }
    };
    configure(options: UiOptions): void {
        if (this.ctrl?.activeCount)
            throw new Error('仍有未确认结束的任务，请先停止或恢复任务后再切换连接。');
        this.teardown();
        this.options = options;
        if (this.isConnected)
            this.mount();
    }
    get controller(): WorkspaceController { if (!this.ctrl)
        throw new Error('先调用 configure 配置客户端。'); return this.ctrl; }
    get uiTheme(): Theme { return this.theme; }
    setTheme(theme: Theme): void { this.theme = theme; this.applyTheme(); this.options?.onThemeChange?.(theme); }
    connectedCallback(): void { if (this.options && !this.ctrl)
        this.mount(); }
    disconnectedCallback(): void { this.teardown(); }
    private teardown(): void {
        this.unsubscribe?.();
        this.unsubscribe = undefined;
        this.ctrl?.dispose();
        this.ctrl = undefined;
        this.themeQuery.removeEventListener('change', this.themeListener);
        this.sizeQuery.removeEventListener('change', this.sizeListener);
        this.removeEventListener('keydown', this.onKeyboard);
        if (this.toastTimer)
            clearTimeout(this.toastTimer);
        if (this.frame)
            cancelAnimationFrame(this.frame);
    }
    private mount(): void {
        const o = this.options!;
        this.ctrl = new WorkspaceController(o);
        this.theme = o.theme ?? 'light';
        this.applyTheme();
        this.themeQuery.addEventListener('change', this.themeListener);
        this.sizeQuery.addEventListener('change', this.sizeListener);
        this.addEventListener('keydown', this.onKeyboard);
        const style = el('link');
        style.rel = 'stylesheet';
        style.href = new URL('./styles.css', import.meta.url).href;
        this.shell = el('div', 'workspace-shell');
        this.shell.dataset.mode = o.mode ?? 'custom';
        this.sidebar = el('aside', 'sidebar');
        this.sidebar.setAttribute('aria-label', '项目与对话');
        const brand = el('div', 'brand', el('div', 'brand-symbol', icon('logo', 25)), el('span', 'brand-name', o.brand ?? 'Mona'), el('span', 'brand-tag', 'WORKSPACE'));
        const sidebarTop = el('div', 'sidebar-brand-row', brand, button('搜索对话', icon('search', 19), () => this.search(), 'icon-button subdued'));
        const newChat = button('新对话', el('span', 'nav-content', icon('edit', 19), '新对话', el('kbd', '', navigator.platform.includes('Mac') ? '⌘ N' : 'Ctrl N')), () => this.newConversation(), 'nav-button active');
        const all = button('全部对话', el('span', 'nav-content', icon('chat', 18), '全部对话'), () => this.search(), 'nav-button');
        const guide = button('接入与定制', el('span', 'nav-content', icon('code', 18), '接入与定制'), () => this.guide(), 'nav-button');
        this.groups = el('div', 'project-groups');
        const groupHeader = el('div', 'section-label', el('span', '', '项目'), button('新建项目', icon('plus', 16), () => nameDialog(this.root, '新建项目分组', '', name => { const id = this.controller.createProject(name); this.controller.newConversation(id); }), 'icon-button small'));
        const foot = el('div', 'sidebar-footer', el('div', 'avatar', (o.brand ?? 'Mona').slice(0, 1)), el('div', 'workspace-account', el('strong', '', '本地工作空间'), el('span', '', '为你的下一步留白')), button('工作空间设置', icon('settings', 19), () => this.settings()));
        this.sidebar.append(sidebarTop, el('nav', 'sidebar-nav', newChat, all, guide), groupHeader, this.groups, el('div', 'sidebar-spacer'), foot);
        const scrim = button('关闭侧栏', null, () => { this.shell.classList.remove('mobile-sidebar'); this.syncSidebarAccessibility(); }, 'sidebar-scrim');
        const panel = el('div', 'workspace-panel');
        this.breadcrumb = el('span', 'breadcrumb-project');
        this.heading = el('span', 'header-title', '新对话');
        const left = el('div', 'header-left', button('切换侧栏', icon('panel', 19), () => { if (matchMedia('(max-width: 760px)').matches)
            this.shell.classList.toggle('mobile-sidebar');
        else
            this.shell.classList.toggle('sidebar-collapsed'); this.syncSidebarAccessibility(); }), el('div', 'breadcrumbs', this.breadcrumb, el('span', 'breadcrumb-slash', '/'), this.heading));
        const modeLabel = o.mode === 'demo' ? '演示模式' : o.mode === 'tauri' ? '本地 Tauri' : o.mode === 'http' ? 'HTTP 连接' : '已配置连接';
        const badge = button('连接设置', el('span', 'connection-inner', el('span', 'status-dot'), modeLabel), () => this.connection(), 'connection-badge');
        this.titleActions = el('div', 'header-actions');
        const right = el('div', 'header-right', this.titleActions, badge, button('运行详情', icon('panel', 18), () => { this.shell.classList.toggle('inspector-open'); if (this.state)
            this.updateInspector(this.state); }));
        const header = el('header', 'workspace-header', left, right);
        const center = el('div', 'center-stage');
        this.thread = el('main', 'thread-scroll');
        this.thread.setAttribute('aria-label', '对话内容');
        this.welcome = el('section', 'welcome');
        const prompts = [{ icon: 'spark', title: '梳理一个想法', caption: '把灵感变成清晰的方向', text: '我有一个新的想法，请帮我梳理目标、使用场景和第一版范围。' },
            { icon: 'code', title: '分析一段代码', caption: '理解结构，找到改进点', text: '请帮我分析下面这段代码的职责、潜在问题和改进建议：\n\n' },
            { icon: 'list', title: '制定执行计划', caption: '将复杂任务拆成下一步', text: '请把我的任务拆解成可执行的步骤，并为每一步给出验收条件：\n\n' }];
        const suggestions = el('div', 'suggestions');
        for (const p of prompts)
            suggestions.append(button(p.title, el('span', 'suggestion-content', icon(p.icon, 20), el('strong', '', p.title), el('span', '', p.caption)), () => this.composer!.fill(p.text), 'suggestion'));
        this.welcome.append(el('div', 'welcome-mark', icon('logo', 53)), el('div', 'welcome-eyebrow', 'A LITTLE CLARITY. A LOT OF POSSIBILITY.'), el('h1', '', o.greeting ?? '我们要一起完成什么？'), el('p', 'welcome-subtitle', o.subtitle ?? '从一个想法开始，把下一步交给 Mona。'), suggestions);
        this.timeline = el('div', 'timeline');
        this.timeline.hidden = true;
        this.cards.clear();
        this.lastConversationId = null;
        this.sidebarKey = '';
        this.thread.append(this.welcome, this.timeline);
        this.thread.addEventListener('scroll', () => { this.scrollButton.hidden = this.thread.scrollHeight - this.thread.scrollTop - this.thread.clientHeight < 130; });
        this.scrollButton = button('跳到最新内容', icon('arrow', 16), () => this.scrollBottom(true), 'scroll-latest');
        this.scrollButton.hidden = true;
        this.composer = new Composer(o, { submit: s => this.controller.send(s), cancel: () => this.perform(async () => { const t = this.controller.active?.turns.at(-1); if (t)
                await this.controller.cancel(t.id); }), templates: () => this.templates(), project: () => this.projectDialog(), permissions: () => this.permissions(), connection: () => this.connection(), notice: s => this.notice(s) });
        const mainColumn = el('div', 'main-column', this.thread, this.scrollButton, this.composer.node);
        this.inspector = el('aside', 'inspector');
        this.inspector.setAttribute('aria-label', '运行详情');
        center.append(mainColumn, this.inspector);
        panel.append(header, center);
        this.toast = el('div', 'toast');
        this.toast.hidden = true;
        this.toast.setAttribute('role', 'status');
        this.toast.setAttribute('aria-live', 'polite');
        this.shell.append(this.sidebar, scrim, panel, this.toast);
        this.root.replaceChildren(style, this.shell);
        this.unsubscribe = this.controller.subscribe(s => this.update(s));
        this.syncSidebarAccessibility();
    }
    private syncSidebarAccessibility(): void {
        const mobile = this.sizeQuery.matches, open = this.shell.classList.contains('mobile-sidebar');
        const hidden = mobile ? !open : this.shell.classList.contains('sidebar-collapsed');
        this.sidebar.inert = hidden;
        this.sidebar.setAttribute('aria-hidden', String(hidden));
        const panel = this.shell.querySelector<HTMLElement>('.workspace-panel');
        if (panel)
            panel.inert = mobile && open;
    }
    private applyTheme(): void { const resolved = this.theme === 'system' ? (this.themeQuery.matches ? 'dark' : 'light') : this.theme; this.setAttribute('data-theme', resolved); }
    private perform(action: () => void | Promise<void>): void { try {
        Promise.resolve(action()).catch(e => this.notice(e instanceof Error ? e.message : String(e)));
    }
    catch (e) {
        this.notice(e instanceof Error ? e.message : String(e));
    } }
    private notice(text: string): void { this.toast.textContent = text; this.toast.hidden = false; clearTimeout(this.toastTimer); this.toastTimer = setTimeout(() => { this.toast.hidden = true; }, 6000); }
    private newConversation(projectId?: string): void { this.perform(() => { this.controller.newConversation(projectId); this.composer?.fill(''); this.shell.classList.remove('mobile-sidebar'); this.syncSidebarAccessibility(); }); }
    private update(state: WorkspaceState): void {
        this.state = state;
        const c = state.conversations.find(c => c.id === state.activeId), project = state.projects.find(p => p.id === (c?.projectId ?? state.projects[0]?.id));
        this.heading.textContent = c?.title ?? '新对话';
        this.breadcrumb.textContent = project?.name ?? '我的工作空间';
        const key = JSON.stringify([state.activeId, state.projects, state.conversations.map(c => [c.id, c.title, c.projectId, c.turns.at(-1)?.phase])]);
        if (key !== this.sidebarKey) {
            this.sidebarKey = key;
            this.updateSidebar(state);
        }
        const changed = this.lastConversationId !== state.activeId;
        const nearBottom = this.thread.scrollHeight - this.thread.scrollTop - this.thread.clientHeight < 140;
        if (changed) {
            this.timeline.replaceChildren();
            this.cards.clear();
            this.lastConversationId = state.activeId;
        }
        const hasTurns = !!c?.turns.length;
        this.welcome.hidden = hasTurns;
        this.timeline.hidden = !hasTurns;
        this.shell.classList.toggle('has-messages', hasTurns);
        const turnIds = new Set(c?.turns.map(t => t.id));
        for (const [id, card] of this.cards)
            if (!turnIds.has(id)) {
                card.node.remove();
                this.cards.delete(id);
            }
        for (const turn of c?.turns ?? []) {
            let card = this.cards.get(turn.id);
            if (!card) {
                card = new TurnCard(this.options!, { retry: () => this.perform(() => this.controller.retryStart(turn.id)), reconnect: () => this.perform(() => this.controller.reconnect(turn.id)), cancel: () => this.perform(() => this.controller.cancel(turn.id)), abandon: () => dialog(this.root, '停止跟踪这个任务？', '这不会取消后端任务，也不会撤销已经发生的操作。', (body, close) => { body.append(el('p', '', '停止跟踪后可以切换连接；请通过其他可信入口核实未结束的任务。'), textButton('确认停止跟踪', () => { this.controller.abandon(turn.id); close(); }, 'secondary-button')); }), notice: s => this.notice(s) });
                this.cards.set(turn.id, card);
                this.timeline.append(card.node);
            }
            card.update(turn);
        }
        this.composer!.update(c?.turns.at(-1), project);
        this.titleActions.replaceChildren();
        if (hasTurns && c)
            this.titleActions.append(button('导出当前对话', icon('download', 17), () => this.export(c), 'icon-button small'), button('对话选项', icon('more', 18), () => this.editConversation(c), 'icon-button small'));
        if (changed || nearBottom) {
            if (this.frame)
                cancelAnimationFrame(this.frame);
            this.frame = requestAnimationFrame(() => this.scrollBottom(false));
        }
        if (this.shell.classList.contains('inspector-open'))
            this.updateInspector(state);
        if (state.notice)
            this.notice(state.notice);
    }
    private updateSidebar(state: WorkspaceState): void {
        const scroll = this.groups.scrollTop;
        const nodes: HTMLElement[] = [];
        for (const project of state.projects) {
            const section = el('section', 'project-section');
            section.append(button(`在 ${project.name} 新建对话`, el('span', 'project-heading-content', icon('folder', 18), el('span', '', project.name), icon('plus', 13)), () => this.newConversation(project.id), 'project-heading'));
            const conversations = state.conversations.filter(c => c.projectId === project.id);
            if (!conversations.length)
                section.append(el('p', 'project-empty', '还没有对话，开始一个新想法'));
            for (const c of conversations) {
                const active = c.turns.at(-1)?.phase;
                const busy = active === 'running' || active === 'starting';
                const b = button(c.title, el('span', 'conversation-content', el('span', 'conversation-name', c.title), busy ? el('span', 'mini-spinner') : null), () => { this.controller.select(c.id); this.shell.classList.remove('mobile-sidebar'); this.syncSidebarAccessibility(); }, `conversation-button${c.id === state.activeId ? ' selected' : ''}`);
                if (c.id === state.activeId)
                    b.setAttribute('aria-current', 'page');
                section.append(b);
            }
            nodes.push(section);
        }
        this.groups.replaceChildren(...nodes);
        this.groups.scrollTop = scroll;
    }
    private scrollBottom(smooth: boolean): void { this.thread.scrollTo({ top: this.thread.scrollHeight, behavior: smooth && !matchMedia('(prefers-reduced-motion: reduce)').matches ? 'smooth' : 'instant' }); this.scrollButton.hidden = true; }
    private export(c: Conversation): void { saveFile(this.controller.exportConversation(c.id), `mona-${c.id.slice(0, 8)}.json`); this.notice('已导出界面记录；不包含模型密钥或内部审计。请妥善保存。'); }
    private editConversation(c: Conversation): void {
        nameDialog(this.root, '对话选项', c.title, name => this.controller.rename(c.id, name), (body, close) => {
            body.append(el('div', 'divider'), textButton('导出对话', () => this.export(c), 'secondary-button'), textButton('删除本地对话', () => this.perform(() => { this.controller.deleteConversation(c.id); close(); }), 'text-button danger'), el('p', 'muted', '删除仅影响本机显示记录，不删除服务端共享任务。'));
        });
    }
    private search(): void {
        dialog(this.root, '搜索对话', '按标题或已发送内容查找。', (body, close) => {
            const input = el('input', 'form-input');
            input.placeholder = '输入关键词…';
            input.setAttribute('aria-label', '搜索关键词');
            input.autofocus = true;
            const list = el('div', 'search-results');
            const draw = () => {
                const q = input.value.trim().toLowerCase();
                const results = this.controller.getState().conversations.filter(c => (c.title + ' ' + c.turns.map(t => t.prompt).join(' ')).toLowerCase().includes(q));
                list.replaceChildren(...results.map(c => button(c.title, el('span', 'search-result-content', icon('chat', 18), el('span', '', c.title)), () => { this.controller.select(c.id); close(); this.shell.classList.remove('mobile-sidebar'); this.syncSidebarAccessibility(); }, 'search-result')));
                if (!results.length)
                    list.append(el('p', 'empty-search', q ? '没有找到相关对话。' : '还没有对话，先开始一个任务吧。'));
            };
            input.addEventListener('input', draw);
            body.append(input, list);
            draw();
        });
    }
    private templates(): void {
        dialog(this.root, '从一个好问题开始', '选中后填入输入框，你可以继续修改。', (body, close) => {
            for (const [name, text] of [['需求分析', '请帮我澄清这个需求的目标、用户、约束和验收标准：\n\n'], ['排查问题', '我遇到了下面的问题，请先列出可能原因，再给出安全的排查步骤：\n\n'], ['整理文档', '请把以下内容整理成结构清晰的说明文档，并标出需要补充的信息：\n\n'], ['代码评审', '请检查以下代码的正确性、错误处理和安全边界：\n\n']])
                body.append(button(name, el('span', 'search-result-content', icon('spark', 17), name), () => { close(); this.composer!.fill(text); }, 'search-result'));
        });
    }
    private projectDialog(): void {
        dialog(this.root, '选择项目分组', '项目仅用于整理对话，不会切换后端工作目录。', (body, close) => {
            for (const p of this.controller.getState().projects)
                body.append(button(p.name, el('span', 'search-result-content', icon('folder', 18), p.name), () => { close(); this.newConversation(p.id); }, 'search-result'));
        });
    }
    private permissions(): void {
        dialog(this.root, '权限由执行宿主管理', '界面不能通过一个开关授予工具权限。', (body) => {
            body.append(el('div', 'info-block', icon('shield', 26), el('p', '', '工具是否可用、能否访问文件或执行命令，由 Core 的最终权限检查与宿主策略决定。')), el('p', 'muted', '本版 UI 不提供“完全访问”切换，也不假装已经实现审批服务。需要审批时，由业务应用接入可信的审批流程。'));
        });
    }
    private connection(): void { if (this.options?.onConnectionRequested)
        this.options.onConnectionRequested();
    else
        this.settings(); }
    private settings(): void {
        dialog(this.root, '工作空间设置', '界面偏好与执行配置彼此独立。', (body, close) => {
            const themes = el('div', 'segmented');
            for (const [id, label, glyph] of [['light', '浅色', 'sun'], ['dark', '深色', 'moon'], ['system', '跟随系统', 'panel']] as const) {
                const b = button(label, el('span', 'segment-label', icon(glyph, 17), label), () => { this.setTheme(id); for (const item of themes.querySelectorAll('button'))
                    item.setAttribute('aria-pressed', String(item === b)); }, 'segment');
                b.setAttribute('aria-pressed', String(this.theme === id));
                themes.append(b);
            }
            body.append(el('div', 'setting-label', '外观'), themes, el('div', 'setting-label', '当前连接'), el('div', 'connection-card', icon(this.options?.mode === 'tauri' ? 'panel' : 'globe', 21), el('div', '', el('strong', '', this.options?.connectionLabel ?? '自定义 AgentClient'), el('p', 'muted', this.options?.mode === 'demo' ? '本地模拟事件，不调用模型。' : '只传输公共流式事件，不向 UI 暴露模型凭据。'))));
            if (this.options?.onConnectionRequested)
                body.append(textButton('配置连接与本地记录', () => { close(); this.options!.onConnectionRequested!(); }, 'secondary-button'));
            body.append(el('div', 'setting-label', '可复用方式'), el('p', 'muted', '一份 Web UI，注入 HTTP、Tauri 或自定义客户端。主题、品牌、请求转换和工具详情都可替换。'));
        });
    }
    private guide(): void {
        dialog(this.root, '从这份界面开始定制', 'UI 是应用层组件，不是另一套 Agent Runtime。', (body) => {
            for (const [title, desc] of [['01 · 选择连接', '在普通浏览器中接入 HTTP Bridge；在 Tauri 中注入 invoke 与 Channel，无需启动 HTTP 服务。'], ['02 · 复用交互', '保留流式消息、工具卡片、停止、补充指令与快照恢复，按业务替换品牌和组件。'], ['03 · 扩展能力', '文件上传、模型选择、审批和会话上下文由宿主接入。默认只发送当前文本任务。']])
                body.append(el('section', 'guide-section', el('h3', '', title), el('p', 'muted', desc)));
            body.append(el('div', 'info-block', el('p', '', '默认记录只存在内存。可在连接设置中明确启用本机存储；它不是后端 Session，也不是跨设备同步。')));
        });
    }
    private updateInspector(state: WorkspaceState): void {
        const c = state.conversations.find(c => c.id === state.activeId), turn = c?.turns.at(-1), snapshot = turn?.snapshot;
        this.inspector.replaceChildren(el('div', 'inspector-header', el('h3', '', '运行详情'), button('关闭运行详情', icon('close', 17), () => this.shell.classList.remove('inspector-open'))));
        if (!turn) {
            this.inspector.append(el('div', 'inspector-empty', icon('terminal', 32), el('p', '', '发送任务后，在这里查看状态。')));
            return;
        }
        const fields = [['状态', snapshot?.outcome ? outcomeLabel(snapshot.outcome) : turn.phase === 'detached' ? '未跟踪 / 结果未知' : turn.phase === 'disconnected' ? '等待恢复' : turn.phase === 'start_unknown' ? '启动待确认' : turn.phase === 'starting' ? '正在启动' : '运行中'], ['Run ID', turn.runId ?? '待分配'], ['事件游标', String(snapshot?.seq ?? 0)], ['决策轮次', String(snapshot?.step ?? 0)], ['协议', 'Stream v2'], ['传输', this.options?.connectionLabel ?? '自定义客户端']];
        for (const [label, value] of fields)
            this.inspector.append(el('div', 'inspector-field', el('span', '', label), el('code', '', value)));
        const outcome = snapshot?.outcome;
        if (outcome) {
            this.inspector.append(el('div', 'inspector-field', el('span', '', '模型请求数'), el('code', '', String(outcome.task_usage.model_calls))), el('div', 'inspector-field', el('span', '', 'Token 用量'), el('code', '', outcome.task_usage.usage_complete ? String(outcome.task_usage.reported_tokens) : '未完整报告')));
            if (turn.runId)
                this.inspector.append(textButton('释放后端任务记录', () => dialog(this.root, '释放后端任务？', '其他客户端将不能再查询这条任务。', (body, close) => { body.append(el('p', '', '当前界面的显示记录保留。此操作只适用于已经结束的任务。'), textButton('确认释放', () => { this.perform(() => this.controller.releaseBackend(turn.id)); close(); }, 'secondary-button')); }), 'secondary-button'));
        }
        this.inspector.append(el('p', 'inspector-note', '只展示公共运行信息。内部提示词、私有推理字段与请求审计不在这里显示。'));
    }
}
