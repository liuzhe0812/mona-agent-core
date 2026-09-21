import { RunView, requestId, BridgeError } from '@agent-core/client';
import type { Subscription, StreamFrame } from '@agent-core/client';
import type { ControllerOptions, WorkspaceState, Conversation, Turn, HistoryStore } from './types.js';
import { restoreHistory, validSnapshot } from './storage.js';
const bytes = (s: string) => new TextEncoder().encode(s).length;
const errorText = (e: unknown) => e instanceof Error ? e.message : '操作未完成，请检查连接。';
/** A UI task coordinator, NOT an Agent loop. Only calls the injected AgentClient. */
export class WorkspaceController {
    private state: WorkspaceState;
    private listeners = new Set<(state: WorkspaceState) => void>();
    private views = new Map<string, RunView>();
    private subscriptions = new Map<string, Subscription>();
    private timer?: ReturnType<typeof setTimeout>;
    private persistTimer?: ReturnType<typeof setTimeout>;
    private disposed = false;
    private history?: HistoryStore;
    readonly maxConversations: number;
    readonly maxTurns: number;
    constructor(readonly options: ControllerOptions) {
        this.maxConversations = Math.min(24, Math.max(1, options.maxConversations ?? 24));
        this.maxTurns = Math.min(24, Math.max(1, options.maxTurns ?? 24));
        this.history = options.history;
        this.state = { projects: options.projects?.length ? structuredClone(options.projects).slice(0, 24) : [{ id: 'default', name: '我的工作空间' }], conversations: [], activeId: null, revision: 0, notice: null };
        try {
            const restored = restoreHistory(this.history?.load());
            if (restored)
                Object.assign(this.state, restored);
        }
        catch {
            this.state.notice = '本地记录未能读取，当前使用内存模式。';
        }
        // One private reducer per execution. History is an untrusted display projection.
        for (const c of this.state.conversations)
            for (const t of c.turns)
                if (t.runId) {
                    const view = new RunView(t.runId);
                    if (t.snapshot) {
                        try {
                            view.apply({ kind: 'snapshot', reason: 'initial', snapshot: t.snapshot });
                        }
                        catch {
                            delete t.snapshot;
                        }
                    }
                    this.views.set(t.id, view);
                }
    }
    getState(): WorkspaceState { return structuredClone(this.state); }
    subscribe(listener: (state: WorkspaceState) => void): () => void {
        this.listeners.add(listener);
        listener(this.getState());
        return () => { this.listeners.delete(listener); };
    }
    get active(): Conversation | undefined { return this.state.conversations.find(c => c.id === this.state.activeId); }
    get activeCount(): number { return this.state.conversations.flatMap(c => c.turns).filter(t => !['done', 'detached'].includes(t.phase)).length; }
    private notify(immediate = true): void {
        if (this.disposed)
            return;
        if (!immediate) {
            this.timer ??= setTimeout(() => { this.timer = undefined; this.notify(); }, this.options.updateIntervalMs ?? 32);
            return;
        }
        if (this.timer) {
            clearTimeout(this.timer);
            this.timer = undefined;
        }
        // Only copy reducer snapshots at a display frame, not for every incoming token.
        for (const c of this.state.conversations)
            for (const t of c.turns) {
                const v = this.views.get(t.id);
                if (v)
                    t.snapshot = structuredClone(v.state);
            }
        this.state.revision++;
        for (const listener of this.listeners)
            listener(this.getState());
        if (this.history) {
            if (this.persistTimer)
                clearTimeout(this.persistTimer);
            this.persistTimer = setTimeout(() => { this.persistTimer = undefined; this.persist(); }, 500);
        }
    }
    private persist(): void {
        try {
            this.history?.save({ format: 'mona.ui.history.v1', projects: this.state.projects, conversations: this.state.conversations, activeId: this.state.activeId });
        }
        catch {
            this.history = undefined;
            this.setNotice('本地保存失败或空间不足。任务仍在运行；可导出对话。');
        }
    }
    setNotice(text: string | null): void { this.state.notice = text; this.notify(); }
    enableHistory(store?: HistoryStore): void { this.history = store; this.notify(); }
    createProject(name: string): string {
        name = name.trim();
        if (!name || name.length > 80)
            throw new Error('项目名需为 1–80 个字符。');
        if (this.state.projects.length >= 24)
            throw new Error('最多保留 24 个项目分组。');
        const id = requestId();
        this.state.projects.push({ id, name });
        this.notify();
        return id;
    }
    newConversation(projectId = this.active?.projectId ?? this.state.projects[0].id): string {
        if (!this.state.projects.some(p => p.id === projectId))
            throw new Error('项目不存在。');
        if (this.state.conversations.length >= this.maxConversations)
            throw new Error('对话数量达到上限，请先导出或删除旧对话。');
        const id = requestId(), now = Date.now();
        this.state.conversations.unshift({ id, title: '新对话', projectId, createdAt: now, updatedAt: now, turns: [] });
        this.state.activeId = id;
        this.notify();
        return id;
    }
    select(id: string): void { if (this.state.conversations.some(c => c.id === id)) {
        this.state.activeId = id;
        this.notify();
    } }
    rename(id: string, name: string): void {
        const c = this.state.conversations.find(c => c.id === id);
        name = name.trim();
        if (!c || !name || name.length > 80)
            throw new Error('对话标题需为 1–80 个字符。');
        c.title = name;
        this.notify();
    }
    deleteConversation(id: string): void {
        const c = this.state.conversations.find(c => c.id === id);
        if (!c)
            return;
        if (c.turns.some(t => !['done', 'detached'].includes(t.phase)))
            throw new Error('请先确认任务已结束；断开连接不等于结束。');
        for (const t of c.turns) {
            this.subscriptions.get(t.id)?.close();
            this.subscriptions.delete(t.id);
            this.views.delete(t.id);
        }
        this.state.conversations = this.state.conversations.filter(c => c.id !== id);
        if (this.state.activeId === id)
            this.state.activeId = this.state.conversations[0]?.id ?? null;
        this.notify(); // Local deletion only. Never delete a shared backend run implicitly.
    }
    async send(text: string): Promise<void> {
        if (this.disposed)
            return;
        const prompt = text.trim();
        if (!prompt)
            return;
        if (bytes(prompt) > 65536)
            throw new Error('任务描述超过 64 KiB，请缩短内容。');
        if (!this.active)
            this.newConversation();
        const c = this.active!;
        const current = c.turns.at(-1);
        if (current && !['done', 'detached'].includes(current.phase)) {
            if (current.phase !== 'running')
                throw new Error('请先恢复或确认当前任务的状态。');
            await this.sendInput(current.id, prompt);
            return;
        }
        if (c.turns.length >= this.maxTurns)
            throw new Error('本对话已达到任务上限，请新建对话。');
        const id = requestId();
        const turn: Turn = { id, prompt, request: { request_id: requestId(), prompt }, phase: 'starting', createdAt: Date.now(), cancelRequested: false, inputs: [] };
        c.turns.push(turn);
        c.updatedAt = Date.now();
        if (c.turns.length === 1 && c.title === '新对话')
            c.title = prompt.slice(0, 28);
        this.notify();
        try {
            if (this.options.prepareStart) {
                const request = await this.options.prepareStart({ prompt, requestId: turn.request.request_id, conversation: structuredClone(c) });
                if (request.request_id !== turn.request.request_id || typeof request.prompt !== 'string' || bytes(request.prompt) > 65536)
                    throw new Error('应用转换器返回了无效请求。');
                turn.request = { request_id: request.request_id, prompt: request.prompt };
            }
        }
        catch (e) {
            turn.phase = 'done';
            turn.error = '请求尚未发送：' + errorText(e);
            this.notify();
            return;
        }
        if (!this.disposed)
            await this.launch(turn);
    }
    private async launch(turn: Turn): Promise<void> {
        turn.phase = 'starting';
        turn.error = undefined;
        this.notify();
        try {
            const response = await this.options.client.start(turn.request);
            if (this.disposed)
                return;
            if (typeof response.run_id !== 'string' || !response.run_id || response.run_id.length > 256)
                throw new BridgeError('无效的任务 ID。', { code: 'protocol' });
            turn.runId = response.run_id;
            turn.phase = 'running';
            this.views.set(turn.id, new RunView(response.run_id));
            this.notify();
            this.attach(turn, 0);
        }
        catch (e) {
            if (this.disposed)
                return;
            // Never create a new id for a network-uncertain start.
            turn.phase = turn.runId ? 'disconnected' : 'start_unknown';
            turn.error = errorText(e);
            this.notify();
        }
    }
    async retryStart(turnId: string): Promise<void> {
        const t = this.find(turnId);
        if (t.phase !== 'start_unknown')
            return;
        await this.launch(t);
    }
    private find(id: string): Turn {
        const t = this.state.conversations.flatMap(c => c.turns).find(t => t.id === id);
        if (!t)
            throw new Error('任务不在当前工作空间中。');
        return t;
    }
    private attach(t: Turn, after: number): void {
        const previous = this.subscriptions.get(t.id);
        previous?.close();
        const subscription = this.options.client.subscribe(t.runId!, { after, onFrame: (frame: StreamFrame) => {
                if (this.disposed)
                    return;
                if (frame.kind === 'snapshot' && !validSnapshot(frame.snapshot))
                    throw new BridgeError('无效的公共快照。', { code: 'protocol' });
                this.views.get(t.id)!.apply(frame);
                if (this.views.get(t.id)!.state.outcome) {
                    t.phase = 'done';
                    t.error = undefined;
                }
                else
                    t.phase = 'running';
                this.notify(t.phase === 'done');
            } });
        this.subscriptions.set(t.id, subscription);
        subscription.closed.then(() => {
            if (this.disposed || this.subscriptions.get(t.id) !== subscription)
                return;
            if (t.phase !== 'done') {
                t.phase = 'disconnected';
                t.error = '订阅已结束，任务状态尚未确认。';
                this.notify();
            }
        }).catch((e: unknown) => {
            if (this.disposed || this.subscriptions.get(t.id) !== subscription)
                return;
            t.phase = 'disconnected';
            t.error = errorText(e);
            this.notify();
        });
    }
    async reconnect(id: string): Promise<void> {
        const t = this.find(id);
        if (!t.runId)
            return;
        try {
            const snapshot = await this.options.client.snapshot(t.runId);
            if (this.disposed)
                return;
            if (!validSnapshot(snapshot))
                throw new BridgeError('无效的公共快照。', { code: 'protocol' });
            const view = this.views.get(t.id) ?? new RunView(t.runId);
            view.apply({ kind: 'snapshot', reason: 'source_resync', snapshot });
            this.views.set(t.id, view);
            t.error = undefined;
            t.phase = snapshot.outcome ? 'done' : 'running';
            this.notify();
            if (!snapshot.outcome)
                this.attach(t, snapshot.seq);
        }
        catch (e) {
            t.phase = 'disconnected';
            t.error = errorText(e);
            this.notify();
        }
    }
    async cancel(id: string): Promise<void> {
        const t = this.find(id);
        if (!t.runId || t.phase === 'done' || t.cancelRequested)
            return;
        t.cancelRequested = true;
        this.notify();
        try {
            await this.options.client.cancel(t.runId); /* Keep observing until the authoritative outcome. */
        }
        catch (e) {
            t.cancelRequested = false;
            t.error = '停止请求未确认：' + errorText(e);
            this.notify();
        }
    }
    async sendInput(id: string, text: string): Promise<void> {
        const t = this.find(id);
        if (!t.runId || t.phase !== 'running')
            throw new Error('任务未运行，无法追加指令。');
        if (bytes(text) > 16384)
            throw new Error('补充指令超过 16 KiB。');
        if (t.inputs.some(i => i.pending))
            throw new Error('上一条补充指令尚未确认。');
        if (t.inputs.length >= 128)
            throw new Error('补充指令达到上限。');
        const input = { id: requestId(), text, applied: false, pending: true, error: undefined as string | undefined };
        t.inputs.push(input);
        this.notify();
        try {
            const r = await this.options.client.input(t.runId, { request_id: input.id, text });
            if (r.request_id !== input.id)
                throw new BridgeError('补充指令回执不匹配。', { code: 'protocol' });
            input.applied = r.applied;
            input.error = r.applied ? undefined : '指令没有进入执行记录。';
        }
        catch (e) {
            input.error = '状态待确认（未自动重发）：' + errorText(e);
        }
        finally {
            input.pending = false;
            this.notify();
        }
    }
    /** Explicit local-only detach. Never claims the backend stopped or completed. */
    abandon(id: string): void {
        const t = this.find(id);
        this.subscriptions.get(id)?.close();
        this.subscriptions.delete(id);
        t.phase = 'detached';
        t.error = '已停止跟踪，后端任务状态未知；这不等于取消任务。';
        this.notify();
    }
    async releaseBackend(id: string): Promise<void> {
        const t = this.find(id);
        if (t.phase !== 'done' || !t.runId)
            throw new BridgeError('只能释放已结束任务。');
        await this.options.client.forget(t.runId);
        this.setNotice('后端任务已释放；当前界面的显示记录保留。');
    }
    exportConversation(id: string): string {
        const c = this.state.conversations.find(c => c.id === id);
        if (!c)
            throw new Error('对话不存在。');
        return JSON.stringify({ format: 'mona.ui.conversation.v1', exportedAt: new Date().toISOString(), conversation: c }, null, 2);
    }
    dispose(): void {
        if (this.disposed)
            return;
        if (this.history)
            this.persist();
        this.disposed = true;
        if (this.timer)
            clearTimeout(this.timer);
        if (this.persistTimer)
            clearTimeout(this.persistTimer);
        for (const s of this.subscriptions.values())
            s.close();
        this.subscriptions.clear();
        this.listeners.clear();
        // Detaching a view is NOT cancellation. The host controls task lifetime.
    }
}
