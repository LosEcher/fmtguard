# fmtguard — 跨项目运行反馈与迭代记录

本文件是 fmtguard 在**各个项目实际运行**后的反馈与迭代决策台账。
与 `.fmtguard/runs.jsonl`（每仓库事件日志，事实源）互补：

- `runs.jsonl` 自动记录每次运行的机械事实（scope / gate / verdict / patch）；
- 本文件记录**人的观察**：哪里好用、哪里误伤、门禁阈值是否合理、需要什么新能力。

## 如何记录

- 用 fmtguard 的项目（含 DSH 会话）遇到问题或好用的场景，往本表追加一行；
- 格式：`| 日期 | 项目 | 场景 | 观察/问题 | 处置/迭代项 |`；
- 迭代项在 [docs/AGENT-GUIDE.md 路线图](AGENT-GUIDE.md) 里编号跟踪（P1b/P2…），
  本表是路线图的**证据输入**；
- 一条反馈对应一个 issue 级粒度：宁可多行，不要一行塞三件事。

## 反馈台账

| 日期 | 项目 | 场景 | 观察/问题 | 处置/迭代项 |
|------|------|------|-----------|-------------|
| 2026-08-20 | fmtguard 自身 | 安装验证（cargo build/test/gates.sh + git/jj 冒烟） | 构建/测试/9 道门禁全过；git 与 jj 场景 end-to-end 正常；幂等、sandbox、replay 均验证 | 无（基线） |
| 2026-08-20 | fmtguard 自身 | 安装路径 | README 原写 `cargo install fmtguard`，但 crate 未发布到 crates.io，该路径不可用；`cargo package` 已可打包 | ① README 已改为 git/path 安装说明；② 待办：crates.io 发布（需 cargo login + publish） |
| 2026-08-20 | fmtguard 自身 | 文档一致性 | README 引用 "the design document"，但仓库内无对应设计文档（docs/ 仅 AGENT-GUIDE.md） | 迭代项：补 DESIGN.md（被拒方案清单已散落在 README/AGENT-GUIDE） |
| 2026-08-20 | unirun | fmtguard 门禁 vs CI cargo fmt --check | **两次 CI 红**：fmtguard intersect 输出与 cargo fmt 在部分构造不一致（if/else 单行待合并、数组元素待竖排）。机制（本地复现）：intersect 只保证"改动范围不新增债"，不保证"文件 rustfmt-clean"；scope 外已有违规时 fmtguard 报 verdict ok / files_changed 0，而 CI 整文件检查照样红。教训：fmtguard 是范围裁剪器，不是格式化裁决者 | ① unirun AGENTS.md 门禁序列已加 `cargo fmt --check` 最终裁决（CI 为准）；② 迭代项 P1b/P2：`--emit json` 增加 out_of_scope_hunks 债务字段 + 新增 `--verify-fmt-check` 模式（apply 后对 scoped 文件跑 rustfmt --check）；③ AGENT-GUIDE 明示职责分工 |
| 2026-09-06 | DSH/plugin_host | 大型 `plugin_host/worker.rs` 增量格式化 | rustfmt 全文件 E3 路径超过默认 30s；fmtguard 按 fail-closed 退出，未写盘、未扩大格式化范围。变更文件会先格式化、再格式化一次做幂等校验，因此大文件最坏付出两次 rustfmt 超时；当前日志只保留“超时”结果，缺少文件规模、阶段耗时与可操作的降级提示 | ① P1c：增加单文件预检/耗时观测，超时错误携带路径、字节数、行数与建议；② P1b：优先试验 E1 rangeFormatting，避免大型文件全量 stdin；③ P2：`--verify-fmt-check` 与 out-of-scope debt 字段继续保留，不能用放宽预算掩盖引擎超时 |
| 2026-09-06 | CanTool | `plugin_host/worker.rs` 多次增量格式化 | `.fmtguard/runs.jsonl` 共 322 个 `fmt_result`，114 次实际改动；其中 7 次 `idempotent=false`，一次明确 `verdict=rejected`。失败集中在自定义 `/tmp/cantool-rustfmt-no-macros` wrapper 运行，说明 formatter/config 稳定性也是独立风险，不应归因于 scope 或预算 | ① agent 指引增加大文件 `--changeset` ranges；② `idempotent=false`、timeout 一律先停手并核对 wrapper/config；③ P1c 记录 formatter 路径、版本、阶段耗时，便于区分工具不稳定与文件规模问题 |
| 2026-09-06 | fmtguard 自身 | P1c 超时诊断 | `EngineError::TimedOut` 原先只有路径，无法判断是文件规模还是 timeout 配置；已增加 bytes/lines/timeout_secs，并在错误消息中给出拆分 ranges 或显式调高 timeout 的建议；默认 fail-closed 不变 | 已落地 `src/engine.rs`；后续仍需 E1 rangeFormatting |
| 2026-09-06 | fmtguard 自身 | P1c 阶段观测 | `fmt_result` 之前无法量化单文件 rustfmt 成本；已增加 `rustfmt_duration_ms`，同时写入事件日志、JSON 报告并由 replay 保留（旧日志缺失字段按 0 兼容） | 已落地 `engine/events/report/replay`；后续可拆分首轮格式化与幂等校验耗时 |
| 2026-09-06 | fmtguard 自身 | P1b 引擎探测 | stable `rustfmt 1.9.0` 对 `--file-lines` 返回 `Unrecognized option`；本机虽有 `rust-analyzer 1.97.0`，但 rangeFormatting 需要 LSP 初始化与 workspace 上下文，不能当作现成 CLI 降级 | E2 暂不接入 stable 默认路径；先设计 RA LSP 会话/超时/幂等与 E3 结果统一协议，再做基准决定默认引擎 |
| 2026-09-06 | fmtguard 自身 | P1b LSP framing | 已新增 `src/lsp.rs`，覆盖 Content-Length 编码、分片读取、多帧缓冲和缺失 header 拒绝；尚未接入 rust-analyzer 子进程或改变 E3 默认路径 | 协议底座已具备；下一步接 initialize/didOpen/rangeFormatting 的短生命周期会话 |
| 2026-09-06 | fmtguard 自身 | P1b LSP 请求形状 | `src/lsp.rs` 已增加 initialize、initialized、didOpen、rangeFormatting、shutdown 构造器及形状单测；仍未启动外部进程，避免未定义生命周期和错误语义进入默认路径 | 下一步实现带 deadline 的子进程会话与响应匹配 |
| 2026-09-06 | fmtguard 自身 | P1b 响应校验 | LSP 层已增加 JSON-RPC `id`/`jsonrpc`/`result` 校验，并拒绝 error response，防止迟到响应污染后续请求；模块尚未接入外部进程 | 下一步实现带 deadline 的读写循环与进程组清理 |
| 2026-09-06 | fmtguard 自身 | P1b Range 坐标 | 已增加 1-based 行区间到 LSP 0-based UTF-16 Range 的转换，中文与 emoji 用例通过；避免把 Rust 字节列号直接发送给 LSP | 下一步接入真实文档快照与 TextEdit 应用 |
| 2026-09-06 | fmtguard 自身 | P1b TextEdit 应用 | `src/lsp.rs` 已实现 TextEdit[] 的 UTF-16→字节安全转换、倒序应用、重叠拒绝和 surrogate 拆分拒绝；仍未启动外部 rust-analyzer | 下一步接入带 deadline 的 LSP 子进程读写循环 |
| 2026-09-06 | fmtguard 自身 | P1b 真实 RA 探针 | 默认 rust-analyzer 对 `rangeFormatting` 返回 `-32600`，提示必须开启 `rustfmt.rangeFormatting.enable`；开启后真实 `initialize → didOpen → rangeFormatting → shutdown` 通过，结果允许为 `null`（无 edits） | E1 仍不可默认启用；需要把配置能力探测、toolchain 约束和 null 结果写入引擎验收 |
| 2026-09-06 | fmtguard 自身 | P1b LSP 会话 | `Session` 已实现 rust-analyzer stdio 启动、stdout reader、响应 ID 匹配、request/notification、响应 deadline、超时 kill 和 shutdown deadline；尚未改变 E3 默认路径 | 下一步用真实 rust-analyzer 做 initialize/didOpen/rangeFormatting/shutdown 探针 |

## 迭代观察清单（供 P1b/P2 排期）

- [ ] crates.io 发布（`cargo publish`，publish-ready 已验证）
- [ ] 补 DESIGN.md：整理被拒替代方案清单（cargo fmt 全量 / --file-lines / tree-sitter / daemon / 散文式 CI 规则）
- [ ] 全局配置文件支持（目前配置全走 CLI flag；跨项目统一预算需重复传参）
- [ ] DSH 插件包装 `rust_fmt_changes` 工具（路线图 P2，已有变更集协议的 agent 入口）
- [x] P2 `--verify-fmt-check`：对待写入 candidate 做整文件 rustfmt clean 检查，scope 外格式债明确 rejected
- [x] P2 `out_of_scope_hunks`：报告、事件日志和 replay 暴露 scope 外被裁剪的 hunk 数量
- [x] P2 sandbox cargo check：隔离 worktree 在 `git diff --check` 后执行 `cargo check --quiet`，失败保持 fail-closed
- [x] P1c 大文件可观测性：记录文件 bytes/lines、单次 rustfmt 阶段耗时与 timeout_secs；超时消息必须包含文件路径和下一步建议
- [x] P1c 阶段耗时拆分：报告首轮格式化、幂等校验与总耗时；旧事件日志按 0 兼容
- [ ] P1c 大文件策略：对超出阈值的文件优先走 rangeFormatting；不可用时明确提示“分拆变更或显式提高 timeout”，仍保持 fail-closed
- [x] P1b 设计稿：RA LSP framing、workspace/range 语义、超时/进程组清理、E3 对照基准与验收故障注入（见 `docs/P1B-RANGE-ENGINE-DESIGN.md`）
