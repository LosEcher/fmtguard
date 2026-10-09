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
| 2026-10-07 | cankey | 大文件增量格式化（`cankey/docs` 25 份提到 fmtguard、18 份提到超时） | `engine.rs`(129 KB)/`pinyin.rs`(111 KB)/`lexicon.rs`/`lm.rs` 稳定超时；`--engine-timeout-secs 120` 后仍跑约 4 分钟才报同一句超时（= 首轮+幂等两次）；同日手工 `cat <file> \| rustfmt --emit stdout --edition 2024` 正常完成；4 处文档写"另案未修"，期间只能用 `apply_patch` 手工对齐 | **P0-1** 管道排空修复（不是调大 timeout）；详见 `docs/ITERATION-CANDIDATES-2026-10-07.md` |
| 2026-10-07 | fmtguard 自身 | P0-1 独立复现 | 170 KB/4000 行：`rustfmt` 单独 **0.065 s**；fmtguard 0.2.0 与 HEAD(0.2.8) 都在 8 s 超时（`user 0.065 s` = 子进程没在算）；根因 `src/engine.rs:158-210` 只对 stdin 起写线程，**stdout/stderr 要等 `try_wait` 拿到状态才排空** ⇒ 子进程写满 64 KiB 管道即阻塞 | P0-1；门禁双臂：≥256 KiB 输入 10 s 内 exit 0，睡死 stub 仍 exit 2 且不写盘 |
| 2026-10-07 | unirun | `docs/ITERATION-CANDIDATES-2026-10-07.md` C11 | 该仓已把同一条路由到 fmtguard，并给出反面参照 `unirun/src/exec.rs:196-201`（两条排空读线程） | 与 P0-1 合并；unirun 侧另加“大输出不阻塞”门禁用例 |
| 2026-10-07 | 全舰队（9 仓台账） | 版本漂移 | 11,076 事件 / 673 run 中 **672 次 `version=0.2.0`**（含 10-07 当天 dsfolder、cantool 的 run），本机 `fmtguard --version`=0.2.0，而 crates.io/仓内=0.2.8 ⇒ `out_of_scope_hunks`、阶段耗时、`--verify-fmt-check`、sandbox cargo check、E1、进程组清理**一个都没到用户手里** | **P1f-1** `fmtguard doctor` + `--require-version`；README/AGENT-GUIDE 升级段；插件 preflight 断言版本 |
| 2026-10-07 | cankey / cantool / canpad / unirun | `ok` 被读成“文件干净” | 2367 次 `fmt_result` 有 **618 次 hunk 被裁掉**（26%）与 `ok` 并存；unirun 两次 CI 红后加 `cargo fmt --check` 终裁；cankey 多份 plan 写“fmtguard 不是格式化裁决者”；实测存量债 cankey **164 hunks/35 文件**、canpad 7/3、grok-build 3/2、fmtguard 自身 23/3 | **P1d-1** 报告自描述：`debt{debt_delta,baseline_debt_hunks,fmt_clean}` + 文本明示“不声明仓干净” |
| 2026-10-07 | canpad | 反向决策 | `survey-20261006.md`/`plan-20261006.md` 明确**不把 `cargo fmt --check` 加进 CI**（fmtguard 是范围裁剪器，整仓检查会把存量债变成不可解红） | P1d 的正确落点：暴露 `debt_delta`，不逼消费仓全量格式化（见候选文档拒绝清单） |
| 2026-10-07 | fmtguard 自身（夹具） | `--verify-fmt-check` 误伤基线债 | `/tmp/fg-debt`：基线债在第 1 行、scope 只覆盖第 27 行且该行格式正确 ⇒ 不带 flag `ok`，带 flag `rejected`（`engine.fmt_check=false`，detail 明写 "scope excludes formatting debt"）；在带债仓里该 flag 等于不可用 | **P1d-2** 分档：默认 `delta`（只禁新增债，基线债计数不阻塞），`strict` 保留现有整文件语义 |
| 2026-10-07 | unirun | 台账保留（P2-3） | `EXECUTION-AUDIT-2026-10-05.md`：598 事件/39 run、0 门禁拒绝、**无保留策略**、28 条 `cwd` 还是已迁移的 `~/syncthing/...`；cankey 2307 事件/764 KB、cantool 6640 事件/1.6 MB | **P1e** `log prune --keep-runs N / --older-than 7d`（默认 dry-run，归档不删） |
| 2026-10-07 | fmtguard 自身 | 失败不留证 | 引擎错误只 `eprintln` 后 return（`src/main.rs:340-352`），**不写事件**；本机超时 run 在台账里只剩 `run_start`/`scope_detect`/`engine_select`；事件也没有 `ts` 字段（unirun 审计只能从 `run_id` 反解时间） | **P1e**：`t:"run_error"` + `report_emit{verdict:"error"}` + 每条事件 `ts`；旧日志按 0/null 兼容 |
| 2026-10-07 | dsfolder（Codex×DSH 分析） | 台账损坏 | `CODEX-DSH-HARNESS-DESIGN-ANALYSIS-2026-08-21.md` P2：“工具台账损坏时 rename 归档而非截断（保留取证）”→ fmtguard/sandbox-run | **P1e**：损坏行 → 整份 rename 为 `runs.jsonl.corrupt-<ts>` 并新开日志 |
| 2026-10-07 | 全舰队 | 预算拒绝诊断不足 | 8 次拒绝：`budget.max_files` 5、`budget.diff_ratio` 2、`engine.idempotent` 1；cantool TODO 证据显示 agent 直接改用 `--budget-max-files 20`；夹具实测：范围外与范围内改动相距 ≤6 行会被 `grouped_ops(3)` 并组，**1 行新增被放大成 63 行** ⇒ `diff_ratio` 拒绝，detail 却只说“超预算” | **P1f-3** 拒绝 detail 带最小合并距离 + `--hunk-context N`（默认 3，可 0） |
| 2026-10-07 | unirun | untracked 新文件静默 | 项目记忆已踩坑（新建 `.rs` 不在 `--scope-from-git` 范围，需 `git add` 或 `--changeset`）；`docs/AGENT-GUIDE.md §7` 至今未写这条 | **P1f-2** `warnings[]` + `--include-untracked`；文档补 |
| 2026-10-07 | cantool | 自定义 rustfmt wrapper 的幂等失败 | 全舰队 7 次 `idempotent=false` 全落在 `plugin_host/worker.rs` + `/tmp/cantool-rustfmt-no-macros`，与范围/预算无关 | 门禁行为正确（保留）；`doctor` 打印 rustfmt 路径/版本，把“wrapper 不稳”与“文件规模”分开 |
| 2026-10-07 | hotpath-rs / gpui-kit / qingjian / grok-build | 未接 fmtguard，走 `cargo fmt --all [--check]` | `CONTRIBUTING.md`/`CLAUDE.md`/`README.md` 里的既有政策；这四仓当前 `cargo fmt --all --check` 为 0（grok-build 3 hunks/2 文件）；全树 ≥64 KiB 的 `.rs` 共 **277 个**（grok-build 195、gpui-kit 22）⇒ P0-1 是潜在风险而非 cankey 独有 | 采用面：P1f-1 doctor + 文档 recipe；**不强制改造别人仓**（拒绝清单） |
| 2026-10-07 | fmtguard 自身 | 载荷卫生 | `cargo package --list` **命中 `.rustopt/runs.jsonl`**（本机运行台账随 `.crate` 发出去）；`.gitignore` 与 `Cargo.toml exclude` 都没管 `.rustopt/` | **P3-payload**；同类先例见 `RUST-CRATE-PAYLOAD-HYGIENE-2026-10-07.md` |
| 2026-10-07 | fmtguard 自身 | 自己的 fmt 债 + CI 无 fmt 门禁 | `cargo fmt --all --check`：`engine.rs` 3 / `lsp.rs` 19 / `main.rs` 1 hunks；`ci.yml` 有 clippy/package/test/gates 但**没有 fmt**；而舰队治理恰恰把“fmtguard 的 `cargo fmt --check` 红”当作负向控制臂 | **P3-self**：自举格式化 + CI 加门禁；**同时必须把 los 侧那条负向控制改指**（否则静默变绿） |
| 2026-10-07 | fmtguard 自身 | 发布面不完整 | GitHub API：`v0.2.0` 1 个 asset，**`v0.2.1`–`v0.2.8` 全 0 asset**；无 publish job、无 tag↔Cargo 断言（unirun 反例 `v0.2.1` 有 tag 无 crate）；工作树还有未提交的 `ci.yml`/`Cargo.toml`/`Cargo.lock` | **P3-release / P3-hygiene**：采用 unirun 的 release+publish job；提交或回退工作树 |

| 2026-10-07 | fmtguard 自身 | P0-1 已修复（v0.2.9） | `src/engine.rs` 改为 spawn 后立刻为 stdout/stderr 各起排空线程（`spawn_pipe_reader`），成功路径 2 s 有界 `recv_timeout`；新增门禁 G11（≈258 KB + 单行 scope：exit 0、ok、<10 s、首轮耗时 >0、只改范围内函数）与 G12（睡死 stub + timeout 2 s → exit 2 不写盘）；负向控制用 `git worktree` 构建修复前的 `b4058ff`：同一夹具 **旧 exit 2 / 10.04 s** vs **新 exit 0 / 75 ms + clip 26 ms**；真实病例 cankey `engine.rs`(129 KB) 从"安装版 30.07 s 超时"变为 **0.18 s ok**，`pinyin.rs`(110 KB) **0.23 s**。`cargo test` 21 passed、G1–G12 全绿 | 已落地；cankey 各 plan 文档里的"另案未修"可撤销 |
| 2026-10-07 | fmtguard 自身 | P1g 裁剪成本连带发现（归因已修） | P0-1 修好后暴露出真凶的另一半：全文件重排 fixture 里 rustfmt 首轮 71 ms、幂等 68 ms，而 **clip（diff+分组+apply）8554 ms**；`rustfmt_duration_ms` 一直记的是整个 `format_file` 的 wall ⇒ 台账把 fmtguard 自己的成本记成 rustfmt 的。实测每翻倍 ≈4×（500 行 0.26 s → 4000 行 8.70 s），且"254 KB + 单行改动"只要 0.20 s（成本由改动量驱动，不是文件大小）；`similar::Algorithm::Patience` 更差（14.3 s），已回退 Myers | 归因已落地（`clip_ms` 进事件/report/replay）；**限幅未做** → P1g 候选：diff deadline + 可归因 fail-closed 错误 |

| 2026-10-07 | fmtguard 自身 | P1f-1 已实现（v0.3.0） | `fmtguard doctor`（read-only：formatter 版本 + **行为探针**、E1/E3 可用性、config/edition、日志健康、`--require-version`）+ 任意运行可加 `--require-version X.Y.Z`（不满足 exit 2，不写盘）。门禁 G13 四臂：正常 exit 0 / `--rustfmt` 不存在 exit 2 且 `probe_ok=false` / `doctor --require-version 9.9.9` exit 2 / 正常 run 带漂移版本 exit 2 且工作树未变、`--require-version 0.0.1` exit 0 | 已落地；建议各仓 AGENTS.md 的命令行加 `--require-version <基线>`，把"装的是旧版"变成门禁 |
| 2026-10-07 | fmtguard 自身 | P1e 已实现（v0.3.0） | 台账生命周期四项全部落地：①每条事件 `ts`（RFC3339 UTC 毫秒，单点注入，测试断言形状与 roundtrip）；②失败留证：`exit 2` 写 `run_error{kind,message,files_failed}` + `report_emit{verdict:"error"}`；③`replay` 新增 `error`/`interrupted`——**悬空 `run_start`（崩溃/被杀）过去会重放成 `ok`，现在报 `interrupted`**；④`log prune --keep-runs N / --older-than 7d`（二选一，默认 dry-run，先归档 `.fmtguard/archive/runs-*.jsonl` 再临时文件+rename 原子替换；`--older-than` 对无 `ts` 的旧日志拒绝而不是猜）；⑤半截写入的日志在下次 append 前整体改名 `runs.jsonl.corrupt-<ts>`（字节保留，新日志健康）。门禁 G14/G15/G16；`cargo test` 26 passed、G1–G16 全绿 | 已落地；unirun `P2-3 台账保留` 与 Codex×DSH 的 P2「损坏 rename 归档」两条外部待办据此可关闭 |

| 2026-10-07 | fmtguard 自身 | P1d 已实现（v0.3.1） | `--verify-fmt-check` 分档：默认 **`delta`** 只拒绝"本次改动范围内新增的债"，范围外既有债只计数；`=strict` 保留旧整文件语义。报告/事件新增 `in_scope_debt_hunks`、`fmt_clean`，`stats.out_of_scope_hunks` 汇总，`ok` 时 stderr 明示 "this verdict does NOT assert repo cleanliness"。夹具实测（基线债在 line 1、scope 只覆盖 line 26）：delta exit 0 / `in_scope_debt_hunks=0` / `fmt_clean=false` / `out_of_scope_hunks=1`，strict exit 1 且 detail 指路 delta；移动 formatter 的 whole-file 夹具下 delta exit 1 且 detail 说 "inside the scoped ranges"。门禁 G17 四臂 | 已落地；unirun `AGENTS.md` 的"fmtguard 报 ok ≠ 文件干净"现在有了机械表述；**不建议**再往消费仓塞 `cargo fmt --all --check` 当唯一门禁 |

| 2026-10-07 | unirun（项目记忆踩坑） | P1f-2 已实现（v0.4.0） | `--scope-from-git` 看不见 untracked `*.rs` 这件事过去只写在技能/记忆里：现在 `git ls-files --others` 的结果进 `scope.untracked`，stderr 打一行 warning（点名文件 + 三条出路），`--include-untracked` 才按整文件纳入范围。门禁 G18 三臂：有 untracked → 警告 + 不在 `scope.files`；加 flag → 真的写盘格式化；无 untracked → 无警告 | 已落地 |
| 2026-10-07 | 全舰队（8 次预算拒绝） | P1f-3 已实现（v0.4.0） | 拒绝 detail 过去只有一句"超预算"。现在 `build_clipped` 额外返回 `kept_lines`/`scope_lines`/`min_cross_gap`，`gates.rs` 用它们写诊断：夹具实测默认 context 下"the kept hunk spans 6 line(s) although the scope declares 1 … pass --hunk-context 0"，`--hunk-context 0` 后同一改动 **verdict ok、min_cross_gap=1**（范围外债被正确裁掉）；紧预算下 detail 报"nearest out-of-scope formatting hunk is 1 line(s) away"。`--hunk-context` 只接受 0–6（>6 直接 exit 2，避免"更大 context 只会并更多"）。门禁 G19 四臂 | 已落地；cantool 那种 `--budget-max-files 20` 的临时放宽应该先试 `--hunk-context` |
| 2026-10-07 | 全舰队 | P1f-4 已实现（v0.4.0） | 预算/排除/rustfmt 路径过去每个 run 重传（cantool 每次 `--budget-max-files 20`、cankey 每次 `--engine-timeout-secs`）。现在 `<repo>/.fmtguard.toml` + `~/.config/fmtguard/config.toml`（TOML 子集解析，未知键/未加引号字符串/`[[table]]` 都 exit 2 并给行号），优先级 default → user → repo → CLI，`--dump-config` 打印**每个键的来源**，`--no-config` 忽略文件。**写盘类开关（`--apply`/`--sandbox`/`--emit`/`--changeset`/`--require-version`）刻意不可来自文件**。门禁 G20 五臂（含"repo 说 strict→真拒绝 / CLI 说 delta→真通过"的行为验证，不只是 dump） | 已落地；各仓终于可以把预算与 `require-version` 基线写进仓 |

| 2026-10-07 | cankey / cantool（jj 仓） | P2-jj 已实现（v0.4.1） | `--apply --sandbox` 过去对 jj 显式 exit 2，而两个最重的消费仓都是 jj。现在 jj 用 `jj workspace add`（工作副本 commit 落在当前 commit 的父上，即改动前状态）+ `cargo check` 验证；`jj diff` 没有 `--check`，空白由 `whitespace.clean` 门禁覆盖；收尾 `jj workspace forget` + `abandon` 孤儿 change，并断言 `jj workspace list` 无残留。门禁 G21 双臂：通过臂（真写主树 + workspace 数 1 + change 数不变）/ 失败臂（`cargo check` 报错 → exit 1、主树字节不变、无残留） | 已落地；cankey/cantool 的 `--scope-from-jj --apply --sandbox` 现在可用 |

| 2026-10-07 | fmtguard 自身（P1g） | 裁剪预算已实现（v0.4.2） | 承接上一条：`--diff-timeout-secs N`（默认 10，0=不限）给 fmtguard 自己的 diff/clip 加预算，超时 **fail-closed exit 2** 并点名"scope clipping hit its Ns budget (M changed line(s))"。实测全文件重排 4000 行：预算 1 s → **1.19 s exit 2 且不写盘**（旧行为：8.7 s 静默算完再被预算门禁拒），254 KB + 单行改动（clip 22 ms）在同预算下 ok。门禁 G22 三臂（预算生效 / 错误不再说 rustfmt / 大文件单行改动不受影响）。配置文件也支持 `diff_timeout_secs` | 已落地 |

| 2026-10-07 | fmtguard 自身（P3-self） | 自举格式化 + CI 加 fmt 门禁（v0.4.2） | 修前 `cargo fmt --all --check` **41 处 diff**（`cli.yml` 里根本没有 fmt 步骤，"格式化门禁工具"自己的仓不满足自己的规矩）。用 fmtguard 自身跑一遍整文件 changeset（13 个 `src/*.rs`，`--budget-max-files 20 --budget-max-added-lines 500`）→ ok / 7 文件 / +238 −71，应用后 `cargo fmt --all --check` **exit 0**；`ci.yml` 在 Clippy 后新增 `Formatting (whole-repo arbiter)` 步骤，负向控制（把违规塞进 `src/main.rs` 的临时副本）exit 1、干净 exit 0 | 已落地。**跨仓后果**：`dsfolder/RUST-REPO-LOS-GOVERNANCE-DESIGN-2026-10-07.md` 把"fmtguard 的 `cargo fmt --check` 红"当作失败闭环的负向控制臂——现在这条臂会静默变绿，**必须改指别的必红门禁**（见候选文档 §8） |

| 2026-10-07 | fmtguard 自身（P3-release） | release workflow 落地（v0.4.2，未打 tag） | 新增 `.github/workflows/release.yml`：tag `v*` 触发，矩阵 4 行（linux-gnu / linux-musl / macos-aarch64 / windows-msvc），资产命名按契约 `fmtguard-<os>-<arch>[.exe]`，**先断言 tag == Cargo.toml 版本**（同族反例：某仓 `v0.2.1` 有 tag 无 crate），构建用 `--profile dist --locked`，**发布前对产物做冒烟**（`--version` + `doctor --emit json` 必须 `probe_ok=true`），最后一个 job 独占 release 创建并校验 4 个资产齐全。本机验证：两个 workflow YAML 解析通过、tag 断言双臂（v0.4.2 OK / v0.9.9 拒绝）、`--profile dist` 构建成功、staged 资产冒烟通过。crates.io 发布仍为手动（需要 token secret），未接进 workflow | 已落地；**下一次打 tag 才真正生效**（v0.2.1–v0.2.8 的 0 asset 无法追溯） |
| 2026-10-07 | 全舰队（尺寸门禁） | 本仓 dist 产物变大，舰队预算已重定基 | 新能力让 `--profile dist` 从 **588,880 → 705,344 B**（+19.8%），按舰队规则（实测 +5%）把 `dsfolder/.rust-los-gov/gates.json` 的 `fmtguard:size:dist` 预算 **620,000 → 741,000**（`measuredBytes` 同步更新，命令串里的 `--budget` 一起改）。用舰队那条命令实测：旧预算 `verdict=fail`（headroom −85,344）→ 新预算 `verdict=pass`（headroom +35,656） | 已落地；**任何消费 `gates.json` 的机器/定时任务无需改代码**，下次运行自洽 |

| 2026-10-07 | fmtguard 自身（P2-plugin） | DSH 插件 `dsh-fmtguard` 落地 | 新仓 `dsplugins/dsh-fmtguard`（host-only bundle，无 client）：工具 `rust_fmt_changes`（changeset 路径/内联 JSON、scope=git/jj、apply、emit、verifyFmtCheck=off\|delta\|strict、预算/hunkContext/diffTimeoutSecs/includeUntracked/requireVersion）+ `fmtguard_doctor`；二进制解析 `config.binary → $FMTGUARD_BIN → PATH → ~/.cargo/bin`，缺失返回安装指引。证据：`node test/preflight.mjs` 在 **web 与 headless 两个 profile** 上用真实 `@deepseek-ai/dsh-tools` 编译 2 个工具通过；`node --test` 冒烟 **6/6**（dry-run ok + 范围外债计数但**不写盘**、预算拒绝=业务结果 exit 1 非 toolError、缺二进制给安装指引、apply 只改范围内一行、doctor ok、doctor 版本门禁 error）；`host-smoke` **2/2**（真宿主编译器注册 2 个工具 + 假二进制回显 argv 验证 `--changeset/--emit/--budget-*/--hunk-context/--include-untracked` 映射与裸名扫 PATH）；两 profile `dsh plugin add`（deps+bundles 自动接线）+ `--dump-config` 行完整；**headless E2E 真调用**返回 `verdict=ok exitCode=0 outOfScopeHunks=1` | 已落地。**首轮 E2E 抓到两个真缺陷**：①`exitCode` 渲染成 `exit ?`（模型照着答 `exitCode: ?`）⇒ 修成 `verdict=… exitCode=… outOfScopeHunks=…` 显式字段行 + 冒烟断言；②解析到的是**旧安装版 0.2.0**（舰队漂移的现场复现）⇒ 顺带把 `~/.cargo/bin/fmtguard` 升到 **0.4.2**（旧的备份在 `/tmp/fmtguard-0.2.0.bak`）。**待办**：web 宿主需重启一次才在 GUI 暴露（`~/.dsh/scripts/dsh-web-restart.sh`，重启后查插件树 `include:fmtguard`） |

| 2026-10-09 | fmtguard 自身（门禁缺口） | 差分门禁 G23 + 分歧清单 G24 | **发现**：22 条门禁里每一条 `cmp -s` 比的都是 fmtguard 与**自己的前后状态**（G1 断言 patch 含格式化后的行、G7 幂等、G8 replay 逐字节、G12/G21/G22 断言"被拒时不写盘"），**没有一条**把产物与 `rustfmt` 对**同一输入的产物**比对——"我们悄悄不再与 rustfmt 一致"这一类失效，现有门禁一条都发现不了。**新增 G23 差分臂**：7 个夹具（struct/match、方法链、macro、`#[rustfmt::skip]`、已干净、注释、长签名）在整文件 scope 下与 `rustfmt --edition 2021`（同一份输入字节，stdin→stdout）**逐字节相等**；外加**负向控制**（changeset 排除该文件时必须 DIFFER，证明比较器不是恒真）与**自覆盖断言**（实际比较臂数必须 == 7，夹具名打错一个字不能让门禁静默缩小）。**新增 `KNOWN-DIVERGENCES.md` + G24**：把两条**故意**的分歧落成数据（KD-1 scope 收敛：范围外字节永不写入；KD-2 拒绝即不写盘：`rustfmt` 会写而我们不写），每条含 subject / input / reference / ours / reason / revive / repro；G24 断言①清单有上界（`KD_MAX=2`，扩容必须是有意为之）②条目数 == 非空 `- reason:` 数③**每条仍然复现**（不再复现 ⇒ 门禁红，强迫人改文档，防文档腐烂）。**元测试（门禁能不能失败）**：把 `RUSTFMT` 指向"`cat` 之后多 echo 一行"的投毒桩 ⇒ 7 个 arm 全部 FAIL 并打印真实 diff、自覆盖断言同时报 `ran 0`；把夹具名打错一个字 ⇒ 报 FAIL（oracle 阶段即失败）而非静默通过。**一处 bash 坑留证**：`local n="$1" d="$G23/$n"` 在 `set -u` 下会因 bash"先展开全部右手边、再赋值"而读未定义的 `n`，已拆成两条并写明原因 | 已落地；全门禁 **G1–G24 全绿**。不改二进制，但 `.crate` 载荷**新增 `KNOWN-DIVERGENCES.md`（2.7 KB，共 24 文件）——有意随包发布**，判据同"docs/AGENT-GUIDE.md 故意保留"：用户应当看得到已知分歧。**未 bump 版本**：无二进制变更，下次发布自然带上 |

## 迭代观察清单（供排期）

> 2026-10-07 跨项目巡检后的分档；详细证据、根因与机械验收见
> [`docs/ITERATION-CANDIDATES-2026-10-07.md`](ITERATION-CANDIDATES-2026-10-07.md)。

**P0（工具不可用级）**

- [x] **P0-1 引擎管道排空**（v0.2.9）：stdout/stderr 在 spawn 后立刻排空；门禁 G11（≈258 KB 输入 <10 s exit 0）+ G12（睡死 stub 仍 exit 2 不写盘）；负向控制用修复前二进制实测 10.04 s 超时

**P1（能力/采用）**

- [x] **P1g** 裁剪 diff 限幅（v0.4.2）：`--diff-timeout-secs` + 可归因的 fail-closed 错误；`clip_ms` 归因；门禁 G22
- [x] **P1f-1** `fmtguard doctor` + `--require-version X.Y.Z`（v0.3.0；门禁 G13 四臂）
- [x] **P1f-2** untracked `*.rs` 警告 + `--include-untracked`（v0.4.0，门禁 G18）
- [x] **P1f-3** 预算拒绝 detail 带 hunk 合并诊断 + `--hunk-context N`（v0.4.0，门禁 G19）
- [x] **P1f-4** 仓内/用户配置文件 + `--dump-config` + `--no-config`（v0.4.0，门禁 G20）
- [x] **P1e** 台账生命周期（v0.3.0）：`ts`、`run_error`、`replay` 的 `error`/`interrupted`、`log prune`、半截日志隔离
- [x] **P1d-1** 报告自描述（v0.3.1）：`in_scope_debt_hunks`/`fmt_clean`/`stats.out_of_scope_hunks` + stderr 明示行
- [x] **P1d-2** `--verify-fmt-check` 分档（v0.3.1）：默认 `delta` / `strict`，门禁 G17 四臂

**P2 / P3**

- [x] **P2-jj** `--apply --sandbox` 支持 jj（v0.4.1，门禁 G21 双臂）
- [x] **P2-plugin** DSH 插件包装（v0.1.0）：`rust_fmt_changes` + `fmtguard_doctor`；preflight/冒烟/host-smoke/headless E2E 全过；**web 待重启激活**
- [x] **P3-release**（v0.4.2）release workflow：4 平台 `--profile dist` 资产 + tag↔Cargo 断言 + 产物冒烟；crates.io 仍需 token（手动）
- [x] **P3-self**（v0.4.2）自举格式化（41→0 diff）+ CI 加 `cargo fmt --all --check`；**los 侧负向控制臂待改指**
- [x] **P3-payload**（v0.4.2）`.rustopt/` 不进 `.crate`（`.gitignore` + `Cargo.toml exclude` + CI 打包门禁模式，含 `ITERATION-CANDIDATES-*`）；`cargo package --list` 实测 23 文件、门禁 PASS
- [x] **P3-docs**（v0.4.2）`docs/DESIGN.md`（含拒绝清单）+ AGENT-GUIDE §7 补 untracked/台账/replay 三条边界

**已完成（保留记录）**

- [x] crates.io 发布（0.2.0 → 0.2.8 已在 crates.io）
- [x] P2 `--verify-fmt-check`：整文件 clean 检查（2026-10-07 发现需分档，见 P1d-2）
- [x] P2 `out_of_scope_hunks`：报告、事件日志和 replay 暴露 scope 外被裁剪的 hunk 数量
- [x] P2 sandbox cargo check：隔离 worktree 在 `git diff --check` 后执行 `cargo check --quiet`，失败保持 fail-closed
- [x] P1c 大文件可观测性：记录文件 bytes/lines、单次 rustfmt 阶段耗时与 timeout_secs；超时消息必须包含文件路径和下一步建议
- [x] P1c 阶段耗时拆分：报告首轮格式化、幂等校验与总耗时；旧事件日志按 0 兼容
- [x] P1c 大文件策略（部分）：E1 `--engine e1` 已可显式启用；**真正的根因见 P0-1**
- [x] P1b 设计稿：RA LSP framing、workspace/range 语义、超时/进程组清理、E3 对照基准与验收故障注入（见 `docs/P1B-RANGE-ENGINE-DESIGN.md`）
