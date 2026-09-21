# 无框架 JavaScript 客户端

根入口只导出协议工具与 RunView；`./http`、`./tauri` 是两个可选适配器，运行时零第三方依赖。TypeScript 声明随包附带。不是 UI 组件库。

作为项目本地依赖安装这个目录，或直接复制 src。包目前 private，不假装已发布 npm。

## 接入思路

1. 选择 HttpAgentClient，或把宿主 Tauri 的 invoke/Channel 注入 TauriAgentClient。
2. 调用 start，给同一个逻辑启动请求保留同一个 request_id。不要每次网络重试就换key。
3. 创建 RunView(run_id)，订阅并在 onFrame 内调用 view.apply(frame)。
4. 渲染 view.state.items 和 outcome；文本与工具结果使用不同显示区域。
5. closed rejected 时读取 BridgeError.cursor，决定重连/取snapshot；显式close仅断订阅，停止任务要调用cancel。

```ts
import {RunView, requestId} from '@agent-core/client';
import {HttpAgentClient} from '@agent-core/client/http';
const client = new HttpAgentClient({baseUrl: 'https://your-agent.example', token: trustedToken});
const {run_id} = await client.start({request_id: requestId(), prompt: '检查任务'});
const view = new RunView(run_id);
const subscription = client.subscribe(run_id, {
  after: 0,
  onFrame(frame) {
    view.apply(frame);
    // 将 view.state 交给你的 React/Vue/其他 UI；不要 innerHTML 拼接模型内容。
  },
});
await subscription.closed;
```

Tauri 用 `new TauriAgentClient({invoke, Channel})` 替换 client 创建即可，其他调用一样。导入 `@tauri-apps/api/core` 是宿主应用的责任，Web客户端不会因此拉入Tauri依赖。

## 恢复与限制

- `after` 是已成功处理的 Run seq，不是 Tauri delivery_id。
- 完成项覆盖已有项，不能把 final text 再追加一次。
- `after` 省略时取当前快照，不重演全部历史动画。
- RunView 不会自动执行工具、请求模型、保存会话或修改工作区。
- Tauri客户端默认120秒无事件报失联，可按业务调整 streamIdleTimeoutMs；桥接默认30秒无ACK丢弃订阅。
- HTTP读取器默认单帧上限128 MiB，允许较大恢复快照；可按部署降低maxEventBytes。并非每次都会分配该上限，但超大UI数据仍需要业务治理。
- UI预览有裁剪与项淘汰，检查各truncated字段和pruned_items；完整归档不是本客户端功能。

## 测试

`node --test test/*.test.mjs`。测试覆盖共同reducer、SSE解码、HTTP适配、Tauri适配/ACK竞态；HTTP/Tauri测试使用受控mock，不等同于Rust后端联调。

开发环境安装TypeScript 5.8+后：`tsc -p tsconfig.json`。示例类型检查不包含你的React/Vue/Tauri完整应用构建。
