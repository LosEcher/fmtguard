# P1b 精确范围格式化引擎设计

## 背景与证据

- 当前 E3 通过 stable `rustfmt --emit stdout` 对整个文件格式化，再按 diff hunk 与调用方范围求交集。
- 变更文件还会再次运行 rustfmt 做幂等校验；大型文件因此可能付出两次全文件成本。
- 本机 `rustfmt 1.9.0-stable` 对 `--file-lines` 返回 `Unrecognized option`，不能把 E2 当作可用降级。
- 本机有 `rust-analyzer 1.97.0`，但它的 rangeFormatting 是 LSP 请求，需要 workspace 初始化和文档版本状态。

## 设计判断

P1b 先实现一个短生命周期的 rust-analyzer LSP 引擎（E1），不引入常驻 daemon，也不改变 E3 的默认行为。E1 只有在能力探针、workspace 初始化、请求响应和二次格式化都成功时才产出结果；任何不确定性回退为明确错误，不静默扩大到 E3。

## 协议边界

1. 启动 `rust-analyzer` 子进程，stdin/stdout 使用 Content-Length LSP framing。
2. 发送 `initialize`，`rootUri` 使用当前仓库根；收到成功响应后发送 `initialized`。
3. 对每个 scoped 文件发送 `textDocument/didOpen`，文本使用当前磁盘快照，版本固定为 `1`。
4. 对每个合并后的范围发送 `textDocument/rangeFormatting`，携带明确的 0-based UTF-16 `Range`。
5. 将返回的 `TextEdit[]` 转换为 fmtguard 内部统一的文本结果，再复用现有 diff、预算、幂等和 apply 门禁。
6. 完成后发送 `shutdown`/`exit` 并等待子进程退出；超时必须杀掉整个进程组。

## Fail-closed 规则

- 找不到 `rust-analyzer`、initialize 失败、响应 JSON 无法解析、版本/范围转换失败、请求超时或 shutdown 未结束：E1 失败，整个 run 不写盘。
- E1 不允许自动扩大 range；相邻编辑需要由调用方在 changeset 中显式声明。
- E1 输出必须与 E3 共用 `FormatResult`，包括 `rustfmt_duration_ms`、幂等结果和 clipped patch，保证 replay 不分叉。
- E1 的二次格式化只验证同一编辑结果是否稳定，不把第二次输出的范围外修改写回。

## 超时与观测

- initialize、每个 rangeFormatting 请求、shutdown 分别计时；总耗时写入现有 `rustfmt_duration_ms`，阶段耗时作为后续事件字段。
- 事件至少记录 engine、rust-analyzer 路径、版本、workspace 根、range 数量和失败阶段。
- 任何超时错误都保留文件路径、bytes、lines 和 timeout；禁止只提高预算掩盖协议或进程生命周期问题。

## 分阶段验收

1. LSP framing 单测：分片读取、Content-Length、错误响应和 EOF。**已完成最小 framing 模块 `src/lsp.rs`，以及 initialize/initialized/didOpen/rangeFormatting/shutdown 请求构造器、响应 ID/错误校验和 1-based→0-based UTF-16 行区间转换；暂未接入引擎。**
2. 最小真实仓库探针：initialize → didOpen → rangeFormatting → shutdown。
   **已完成 TextEdit[] 安全应用层和 `Session` 子进程 API：stdout reader、响应 ID 匹配、请求 deadline、超时 kill、shutdown deadline；尚未接入 fmtguard 默认引擎。**
3. 与 E3 对照：相同输入的 scoped patch、幂等结果和 `git diff --check` 一致。
4. 故障注入：进程不存在、响应超时、坏 JSON、错误 range、子进程残留；全部验证 fail-closed。
5. 大文件基准：以 `plugin_host/worker.rs` 为样本，比较 E1/E3 的 p50/p95 耗时、范围外改动数和 patch 字节数。

真实探针已验证 `initialize → didOpen → rangeFormatting → shutdown`。rust-analyzer 默认会以 `-32600` 拒绝 rangeFormatting；必须在 initialize 的 `settings` / `initializationOptions` 中显式开启 `rustfmt.rangeFormatting.enable`。开启后，合法结果可以是 `null`（表示范围内无需编辑），不能强制要求数组非空。

## 暂不做

- 不实现 `--file-lines` stable 兼容层；该能力由 toolchain 决定，探针失败不能伪装成功。
- 不引入常驻 rust-analyzer daemon、跨 run 缓存或 workspace 共享状态。
- 不在本阶段改变默认 engine；默认切换必须由真实基准和回放一致性共同批准。
