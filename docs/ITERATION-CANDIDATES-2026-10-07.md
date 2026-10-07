# fmtguard 迭代候选（2026-10-07 跨项目巡检）

> 状态：**proposed**（本文件只做立项与验收定义，不改代码）
> 输入：9 份 `<repo>/.fmtguard/runs.jsonl` 台账 + 各仓 plan/audit 文档 + 本机实测复现
> 上位文档：[docs/FEEDBACK.md](FEEDBACK.md)（人的观察台账）、[docs/AGENT-GUIDE.md §10](AGENT-GUIDE.md)（路线图）

## 0. 结论（先看这一段）

跨项目台账汇总（9 个仓、11,076 事件、673 次 run）：

| 口径 | 实测值 |
|------|--------|
| run 数 / verdict | 673 runs；`ok` 562、`rejected` 8（**无一次 error 事件**——见 P1e） |
| 运行版本 | `0.2.0` 672 次、`0.1.0` 1 次 ⇒ **全舰队 100% 跑的是 0.2.0**（crates.io/仓内已 0.2.8） |
| `fmt_result` / 真的改了 | 2367 / 767 |
| `idempotent=false` | 7（全部是 cantool `plugin_host/worker.rs` + 自定义 rustfmt wrapper） |
| scope 外 hunk 被裁掉的文件次数 | **618（占 fmt_result 的 26%）** |
| 门禁拒绝构成 | `budget.max_files` 5、`budget.diff_ratio` 2、`engine.idempotent` 1 |
| 全树 ≥64 KiB 的 `.rs` | **277 个**（grok-build 195、gpui-kit 22、cankey 6、cantool 6、hotpath-rs 5、cantool-dep-ws 4、canpad 1、terminal-browser 1） |

**五件事值得进迭代计划**（其余为杂项/已闭环；执行进度就地更新）：

1. **P0-1 大文件"超时"是管道自锁死，不是 rustfmt 慢**（已独立复现，两个版本都有）——**✅ 已修（v0.2.9，含 G11/G12 门禁与负向控制）**；
2. **P1d 格式债语义**：`ok` 被读成"文件干净"，而 `--verify-fmt-check` 在带债仓里把**别人留下的债**判给当前调用方；
3. **P1e 台账生命周期**：无时间戳、失败（exit 2）不留证据、无保留策略、损坏时无归档；
4. **P1f 采用面**：版本漂移（全舰队 0.2.0）、untracked 新文件静默、预算拒绝的诊断不足、无配置文件；
5. **P1g 裁剪成本超线性 + 归因**（P0-1 的连带发现）：全文件重排 4000 行时 clip 占 8.55 s / 8.70 s——归因已修（`clip_ms`），限幅未做。

---

## 1. 证据基础（口径与复跑）

- 台账：`cankey` 2307 行、`cantool` 6640、`canpad` 900、`unirun`、`sandbox-run`、`fmtguard`、`dsfolder`、`cantool/src-tauri`、`cantool-dep-ws`（共 9 份）。
- 文档：`cankey/docs/plan/funnel-attribution-2026-09-29.md §6`（>64 KiB 不可用，唯一一次带详细机制的记录）、`cankey/docs/plan/{full-audit-2026-10-01,architecture-review-2026-10-04,remediation-batches-2026-09-23,remaining-work-2026-10-01}.md`、`unirun/AGENTS.md`、`unirun/docs/EXECUTION-AUDIT-2026-10-05.md §5`、`unirun/docs/ITERATION-CANDIDATES-2026-10-07.md C11`、`canpad/docs/{survey,plan}-20261006.md`、`dsfolder/{RUST-REPO-LOS-GOVERNANCE-ANALYSIS,RUST-CRATE-PAYLOAD-HYGIENE,RUST-BUILD-LOOP-HARNESS-PLAN}-2026-10-0*.md`、`dsfolder/CODEX-DSH-HARNESS-DESIGN-ANALYSIS-2026-08-21.md`。
- 本机实测（2026-10-07）：`/tmp/fg-repro`（170 KB 大文件）、`/tmp/fg-debt`（基线债夹具）、各仓 `cargo fmt --all --check`、`cargo package --list`。命令见 §5。

---

## 2. P0-1 引擎管道自锁死：大文件被误报为「rustfmt 超时」

> **状态：✅ 已实现（2026-10-07，v0.2.9）**
> - `src/engine.rs`：stdout/stderr 各自在 spawn 后立刻起排空线程（`spawn_pipe_reader`），主循环只做 `try_wait` + deadline；成功路径用 2 s 有界的 `recv_timeout` 收结果（防孙进程持 fd），超时路径 kill → reap → 不 join。
> - 门禁：`test/gates.sh` 新增 **G11**（≈258 KB 输入 + 单行 scope：exit 0、verdict ok、**<10 s**、`rustfmt_first_pass_ms > 0`、apply 只改范围内函数）与 **G12**（`--rustfmt` 睡死 stub + `--engine-timeout-secs 2`：exit 2、不写盘）。
> - 负向控制（`git worktree` 检出修复前的 `b4058ff` 构建对照）：同一 257,760 B 夹具、同样 `--engine-timeout-secs 10` ⇒ **旧二进制 exit 2 / 10.04 s**，修复后 **exit 0 / rustfmt 75 ms + clip 26 ms**，G11 的 0.2 s 通过。
> - 真实病例回归：cankey `crates/cankey-core/src/engine.rs`（129,541 B，edition 2024 + cankey `rustfmt.toml`）⇒ 修复前安装版 0.2.0 **exit 2 / 30.07 s**，修复后 **exit 0 / 0.18 s**（3 个 hunk 全在 scope 外、`ok`）；`pinyin.rs`（110,906 B）**0.23 s** 通过。
> - 顺带的归因修复：`fmt_result`/报告/replay 新增 **`clip_ms`**，把 fmtguard 自己的 diff/clip 成本从 `rustfmt_duration_ms` 里分离出来（见 §6 P1g）。

**问题**：含 ≥64 KiB 格式化输出的文件，fmtguard 稳定 `TimedOut`（exit 2，fail-closed，不写盘），而同一个文件直接喂 `rustfmt` 是毫秒级。

**跨项目证据**
- cankey：`engine.rs` 129,541 B、`pinyin.rs` 110,906 B、`lexicon.rs`、`lm.rs` 反复超时；`--engine-timeout-secs` 从 30 提到 120 后**跑了约 4 分钟**仍报同一句超时（= 首轮 + 幂等校验各 120 s）；`cat <file> | rustfmt --emit stdout --edition 2024` 手工执行正常完成。`cankey/docs` 里 **25 份文档提到 fmtguard、18 份同时提到超时**，其中 4 处明确写"另案未修"，期间只能用 `apply_patch` 手工对齐。
- unirun：`docs/ITERATION-CANDIDATES-2026-10-07.md` C11 已把它路由到 fmtguard，并给出反面参照（unirun `src/exec.rs:196-201` 的两条排空读线程）。

**本机复现（今日）**

| 命令 | 结果 |
|------|------|
| `rustfmt --emit stdout --edition 2021 big.rs >/dev/null`（170 KB，4000 行） | **0.065 s** |
| `fmtguard --changeset cs.json --engine-timeout-secs 8`（安装版 0.2.0） | 8.038 s 后 `rustfmt timed out on big.rs`，`user 0.068 s` |
| 同上（HEAD 构建 0.2.8） | 8.077 s 后超时，`user 0.065 s` |
| 同一 changeset 换 31 B 小文件 | 0.065 s，verdict ok |

**根因（高置信）**：`src/engine.rs:158-210` 只对 **stdin** 起了写线程；**stdout/stderr 直到 `try_wait()` 返回状态后才 `read_to_end`**（`:186-192`）。子进程先把 stdout 写满 64 KiB 管道缓冲区就阻塞，再也不会退出 ⇒ `try_wait` 永不返回 ⇒ 到 deadline 报超时。"提高 timeout 无效"与"user CPU ≈ 0"都只有这一种解释。

**建议实现**：spawn 后立刻为 stdout、stderr 各起一条排空线程（stdin 写线程保留），主循环只做 `try_wait` + deadline；超时路径 kill 后 join 读线程，避免把"排空失败"当成工具错误。参考 unirun `src/exec.rs` 的同形实现。

**机械验收**
- `bash test/gates.sh` 新增门禁：≥256 KiB 输入 + 单行 scope，**10 s 内 exit 0**，且 `fmt_result.rustfmt_first_pass_ms` 有值（正臂）。
- 负臂：`--rustfmt <睡 30 s 的 stub>` + `--engine-timeout-secs 2` 必须 **exit 2 且不写盘**（超时语义不能被修死锁的改动顺手弄没）。
- 复跑：`/tmp/fg-repro` 的 `big.rs` 用例改为仓内夹具（生成式，不入库大文件）。

**风险**：低。改动局限在 `run_with_timeout`；幂等门禁与 E3 语义不变。

---

## 3. P1d 格式债语义：`ok` ≠ 干净，`--verify-fmt-check` 又会替别人背债

> **状态：✅ 已实现（2026-10-07，v0.3.1）** —— `--verify-fmt-check` 默认 `delta`（只拒绝范围内新增债；
> 范围外既有债计数上报），`=strict` 保留整文件语义；报告/事件新增 `in_scope_debt_hunks`/`fmt_clean`，
> `stats.out_of_scope_hunks` 汇总；`ok` 时 stderr 明示 "this verdict does NOT assert repo cleanliness"。
> 门禁 **G17** 四臂：无 flag（无新字段）/ delta 在带债仓 exit 0 / strict 同夹具 exit 1 且指路 delta /
> 移动 formatter 下 delta exit 1 且 detail 说 "inside the scoped ranges"。

**问题 A（误读）**：`verdict: ok` 只声明"改动范围内没新增债"，不声明"文件/仓库 rustfmt-clean"。台账里 **618 次 fmt_result 有 hunk 被裁掉**，`ok` 与大量被丢弃的格式化并存。

- unirun 因此两次 CI 红，`AGENTS.md` 被迫补一条终裁：`cargo fmt --check`（并写明"fmtguard 报 ok ≠ 文件干净"）。
- canpad 反向决策：`survey-20261006.md` / `plan-20261006.md` 明确**不把 `cargo fmt --check` 加进 CI**，因为 fmtguard 是范围裁剪器，整仓检查会把存量债变成不可解红。
- 实测存量债（今日 `cargo fmt --all --check`）：cankey **164 hunks / 35 文件**、canpad 7/3、grok-build 3/2、**fmtguard 自己 23/3**；cantool / hotpath-rs / gpui-kit / qingjian / terminal-browser 为 0。

**问题 B（误伤）**：`--verify-fmt-check` 对"写入后的整文件"重新 rustfmt 并要求逐字节相等，于是**范围外、调用方从未碰过的存量债**会把这次 run 判死。

- 夹具实测（`/tmp/fg-debt`，基线债在第 1 行，scope 只覆盖第 27 行且该行格式正确）：
  - 不带该 flag：`verdict: ok`；
  - 带该 flag：`verdict: rejected`，`engine.fmt_check = false`，detail `candidate is not rustfmt-clean; scope excludes formatting debt`。

**建议实现**
1. 报告自描述：JSON 增加 `debt: {out_of_scope_hunks, baseline_debt_hunks, debt_delta, fmt_clean: true|false|null}`（`null` = 未做 clean 检查，避免下游把"没检查"读成"干净"）；文本输出一行 `scope-clipped formatter: N hunks dropped in M files; this verdict does NOT assert repo cleanliness`。
2. `--verify-fmt-check` 语义分档：默认 `delta`——把 `base_ref` 的版本内容 rustfmt 成基线，要求 candidate **不新增**相对基线的偏差，基线债如实计数、不阻塞；`--verify-fmt-check=strict` 保留现有整文件语义（供已清零的仓使用）。
3. AGENT-GUIDE/README 写清分工：fmtguard = 范围裁剪 + 预算门禁；整仓干净性归 `cargo fmt --all --check`（或未来的 `fmtguard debt` 视图）。

**机械验收**（`test/gates.sh` 夹具，双臂）
- 带基线债 + 范围内改动格式正确：`delta` 模式 **exit 0**，`debt_delta=0`、`baseline_debt_hunks>0`；`strict` 模式 exit 1（把现有语义钉住）。
- 构造"裁剪后仍不是 rustfmt 不动点"的候选（相邻 hunk 合并导致的半套改动）：`delta` 模式 **exit 1**。
- `--emit json` 字段存在性 + `replay` 旧日志（无 `debt` 字段）按 `null` 兼容。

**风险**：中。涉及报告字段与门禁语义，需两臂控制；旧日志/旧消费方必须能按 `null` 兼容。

---

## 4. P1e 台账生命周期：没有时间、没有失败、没有上限、损坏无归档

> **状态：✅ 已实现（2026-10-07，v0.3.0）** —— `ts` 单点注入（`events::append`）、失败写
> `run_error{kind,message,files_failed}` + `report_emit{verdict:"error"}`、`replay` 新增
> `error`/`interrupted`（悬空 run 不再报 `ok`）、`log prune --keep-runs N | --older-than 7d`
> （默认 dry-run；先归档 `.fmtguard/archive/runs-*.jsonl` 再 temp+rename 原子替换；旧日志缺 `ts`
> 时 `--older-than` 拒绝）、半截写入的日志改名 `runs.jsonl.corrupt-<ts>`。门禁 **G14/G15/G16**。

| 缺口 | 证据 | 建议 |
|------|------|------|
| 事件无 `ts` | 全部事件只有 `run_id`（epoch 藏在 id 里）；unirun 审计只能从 `run_id` 反解时间（`EXECUTION-AUDIT-2026-10-05.md` 附录） | 每条事件加 RFC3339 毫秒 `ts`；旧日志靠 `run_id` 回退 |
| 失败不留证据 | 引擎错误只 `eprintln` 后 return（`src/main.rs:340-352`），**不写任何事件**；本机复现的超时 run 在台账里只剩 `run_start`/`scope_detect`/`engine_select`。FEEDBACK 里 P1c 那条也只能写"日志只保留超时结果" | 新增 `t:"run_error"`（kind/message/bytes/lines/timeout_secs）+ `report_emit{verdict:"error"}`，replay 能打出失败原因 |
| 无保留策略 | unirun `P2-3`（598 事件/39 runs、28 条 `cwd` 仍是旧的 `~/syncthing/...`）；cankey 2307 事件/764 KB；cantool 6640 事件/1.6 MB；cankey 把 `.fmtguard/` 当 7 天证据窗口 | `fmtguard log prune --keep-runs N` / `--older-than 7d`，**默认 dry-run**，归档到 `.fmtguard/archive/`（不直接删）；`--log` 文案同步 |
| 损坏即污染 | `dsfolder/CODEX-DSH-HARNESS-DESIGN-ANALYSIS-2026-08-21.md` P2："工具台账损坏时 rename 归档而非截断（保留取证）→ fmtguard/sandbox-run" | 解析失败的行 → 把整份日志 rename 为 `runs.jsonl.corrupt-<ts>` 并新开日志，绝不在坏文件里继续追加 |
| `cwd` 记录旧路径 | 同上（28 条 `~/syncthing/...`，仓已迁 `~/syncfolder/...`） | 只记当次真实 `cwd`（现状即如此）；prune 时可按 run 归档，不重写历史 |

**机械验收**：超时夹具跑完后 `jq -e 'select(.t=="run_error")'` 有命中且 `replay` 打印原因；`log prune --keep-runs 1` 在 3-run 夹具上保留最新 run 且 `replay` 字节一致、旧 run 出现在 `archive/`；损坏文件夹具 → rename 发生、原文件仍在、下一次 run 写新文件。

---

## 5. P1f 采用面：装的是 0.2.0、新文件看不见、拒绝说不清、没有配置

1. **版本漂移（全舰队）**（**✅ 工具侧已给出门禁，v0.3.0**：`doctor` + `--require-version`）：`run_start.version` 672/673 是 `0.2.0`（含今天 dsfolder 与 cantool 的 run），本机 `fmtguard --version` = 0.2.0，而 crates.io/仓内 = 0.2.8 ⇒ `out_of_scope_hunks`、阶段耗时、`--verify-fmt-check`、sandbox `cargo check`、E1、进程组清理**一个都没到用户手里**。`RUST-REPO-LOS-GOVERNANCE-ANALYSIS-2026-10-07.md` 也把这条列为 finding #1。剩余动作在消费方：各仓 AGENTS.md 命令加 `--require-version`，并升级本机/CI 的二进制。
   - 建议：`fmtguard doctor [--emit json]`（版本、rustfmt 版本、配置探测、E3/E1 可用性、日志可写、可选 crates.io 最新版比对、`--require-version X.Y.Z` → 漂移 exit 2）；README/AGENT-GUIDE 补升级段；DSH 插件 preflight 里断言版本。
   - 验收：`doctor` 在缺 rustfmt 时 exit 2、正常 exit 0；`--require-version 9.9.9` exit 2。
2. **untracked 新文件静默**（**✅ v0.4.0**：`scope.untracked` + warning + `--include-untracked`）：`--scope-from-git` 只看 tracked diff（unirun 项目记忆已踩过；AGENT-GUIDE §7 至今没写）。新建 `.rs` 文件是最常见的 agent 动作之一，结果是 `nothing to do` 被当成"格式化过了"。
   - 建议：`scope_detect` 发现 scope 外存在 untracked `*.rs` 时输出 `warnings[]` + stderr 一行；新增 `--include-untracked`。
   - 验收：夹具（untracked .rs）→ 警告存在且退出码不变；加 `--include-untracked` 后文件进入 scope；无 untracked 时无警告（负向控制）。
3. **预算拒绝的诊断太薄 + hunk 合并退化**（**✅ v0.4.0**：`--hunk-context 0-6` + detail 里的合并诊断 + `min_cross_gap`）：8 次拒绝里 5 次 `max_files`、2 次 `diff_ratio`；cantool TODO 里的证据是 agent 直接改用 `--budget-max-files 20`。实测机制：范围外改动与范围内改动相距 ≤6 行会被 `grouped_ops(3)` 并成一个 group（本机夹具里 1 行新增被放大成 63 行 ⇒ `diff_ratio` 拒绝），detail 只说"超预算"，没说是**合并**导致的。
   - 建议：拒绝 detail 附带最小合并距离与涉及文件；新增 `--hunk-context N`（默认 3，允许 0）把合并距离变成调用方可控参数；文档补充"密集未格式化区域里 scope 隔离会退化"的边界。
   - 验收：夹具断言默认 detail 含合并诊断；`--hunk-context 0` 时相邻 1 行外的外部改动不被合并（patch 不含外部行，`out_of_scope_hunks` 相应增加）。
4. **无配置文件**（FEEDBACK 未决项；**✅ v0.4.0**：`.fmtguard.toml` / 用户配置 + `--dump-config` + `--no-config`）：预算/排除/rustfmt 路径每仓重传（cantool 每次 `--budget-max-files 20`、cankey 用 `--engine-timeout-secs`）。
   - 建议：`<repo>/.fmtguard.toml` + `~/.config/fmtguard/config.toml`，CLI 覆盖文件；`--dump-config` 打印来源（verify-gate 先例）。
   - 验收：文件设 `max_files=20` 生效，CLI 覆盖生效，`--dump-config` 标出来源。

---

## 5.5 P1g 裁剪成本随「改动量」超线性增长（P0-1 的连带发现）

> **状态：✅ 已实现（2026-10-07，v0.4.2）** —— `clip_ms` 归因（v0.2.9）+ `--diff-timeout-secs N`
> （默认 10，0=不限）限幅：超预算 **fail-closed exit 2** 且错误点名 clip 预算，绝不退化成粗粒度 patch；
> 实测全文件重排 4000 行：1 s 预算 → 1.19 s exit 2（旧：8.7 s 后才被预算门禁拒），
> 254 KB + 单行改动照常 0.2 s ok。门禁 **G22** 三臂。

**问题**：fmtguard 的 `rustfmt_duration_ms` 一直是"整个 `format_file` 的 wall time"，但真正贵的不是 rustfmt。实测（本机，每次 pass 都单独计时）：

| 输入 | 行数 | rustfmt 首轮 | 幂等轮 | **clip（diff+分组+apply）** | wall |
|------|------|--------------|--------|------------------------------|------|
| 20 KB 全文件重排 | 500 | 47 ms | 47 ms | ~180 ms | 0.26 s |
| 41 KB 全文件重排 | 1000 | 46 ms | 51 ms | ~590 ms | 0.68 s |
| 84 KB 全文件重排 | 2000 | 47 ms | 49 ms | ~2.18 s | 2.27 s |
| 170 KB 全文件重排 | 4000 | 71 ms | 68 ms | **8.55 s** | 8.70 s |
| 254 KB 单行改动 | 4000 | 76 ms | 75 ms | ~50 ms | **0.20 s** |

- 成本由**改动行数**驱动（每翻倍 ≈4×，Myers 的 O(N·D)），与文件大小无关：真实的"大文件 + 小改动"路径只要 0.2 s。
- 试过 `similar::Algorithm::Patience`：**更差**（mass4000 14.3 s vs 8.7 s），已回退默认 Myers。
- 危害有两层：①`rustfmt_duration_ms` 把 8.7 s 记成 rustfmt 的成本（**已修**：新增 `clip_ms`，实测 total 8700 = 71+68+8554）；②超大改动量会让 wall time 靠分钟走（`engine.timeout` 只管 rustfmt，管不到这里），使用者看到的是"慢"，且没有一条"改动量太大、scope 隔离算不动"的显式信号。

**建议实现**：给裁剪的 diff 加 deadline（`--diff-timeout-secs`，默认与 engine timeout 同量级），超时 **fail-closed** 报出可归因的错误（`diff too large: N changed lines; split the change`），不要静默退化成粗粒度 patch（那会让改动跑到声明范围外）；并在报告里给出 `changed_lines` 便于判据。

**机械验收**：全文件重排夹具要么在 deadline 内完成，要么 exit 2 且错误信息指向 diff 成本（不得再出现 "rustfmt timed out" 字样）；正臂（大文件+单行改动）保持 <1 s。

## 6. 其余候选（P2/P3，低风险或已在路线图）| # | 项 | 证据 | 验收 |
|---|----|------|------|
| P2-jj ✅ v0.4.1 | `--apply --sandbox` 支持 jj（`jj workspace add` + `cargo check` + forget/abandon） | cankey/cantool 都是 jj 仓（cantool 413 次 `--scope-from-jj`）；sandbox-run 已有 `jj workspace add/forget/abandon` 语义可复用 | jj 夹具：sandbox 通过则写主树；失败则主树不变且 `jj workspace list` 无残留 |
| P2-plugin ✅ v0.1.0 | DSH 工具包装 `rust_fmt_changes`（+ `fmtguard_doctor`） | 路线图 P2；全舰队目前靠 shell 手敲命令 | 新仓 `dsplugins/dsh-fmtguard`；preflight（web+headless）2/2、冒烟 6/6、host-smoke 2/2、**headless E2E 真调用** `verdict=ok exitCode=0 outOfScopeHunks=1`；web 重启后激活 |
| P3-release ✅ v0.4.2 | 补 release workflow（v0.2.1–v0.2.8 的 GitHub release **0 assets**） | GitHub API 实测 | `.github/workflows/release.yml`：4 平台 `--profile dist` + `<tool>-<os>-<arch>` 资产 + tag==Cargo 断言 + 资产冒烟 + 资产齐全校验；本机 YAML/断言/dist 构建/冒烟四验；**下次打 tag 生效** |
| P3-self ✅ v0.4.2 | 自身源码不 rustfmt-clean，且自己的 CI 没有 fmt 门禁 | 修前实测 **41 处 diff**（13 个 `src/*.rs`） | fmtguard 自举整文件 changeset → ok / 7 文件 / +238 −71 → `cargo fmt --all --check` exit 0；`ci.yml` 加 fmt 步骤，负向控制 exit 1 |
| P3-payload | `.rustopt/runs.jsonl` **会进 .crate 载荷**（`cargo package --list` 命中；`.gitignore` 与 `exclude` 都没管它） | 本机实测；同类问题见 `RUST-CRATE-PAYLOAD-HYGIENE-2026-10-07.md`（sandbox-run tracked `.fmtguard/`） | `cargo package --list` 不再命中；CI 打包门禁加该模式 |
| P3-hygiene | 本次之外的工作树仍未提交：`ci.yml`（打包门禁）、`Cargo.toml`（exclude）、`Cargo.lock` | `git status`；`cargo publish` 要求干净树 | 提交或回退，使"仓内状态 = 将要发布的载荷" |
| P3-docs ✅ v0.4.2 | 仓内缺 `docs/DESIGN.md`；AGENT-GUIDE §7 缺已知边界 | FEEDBACK 未决项 + 本次实测 | `docs/DESIGN.md`（分层/契约/决策/拒绝清单/已知限制）+ AGENT-GUIDE §7 补 untracked、`replay` 三态、台账隔离与保留；README 链接可达并被 CI 打包门禁保护 |

---

## 7. 拒绝清单（一行理由）

| 不做 | 理由 |
|------|------|
| 把 `cargo fmt --check` 塞进每个消费仓 CI | 会把存量债变成 agent 不可解的红（canpad 已明确拒绝）；该做的是 fmtguard 暴露 `debt_delta`，不是逼消费仓全量格式化 |
| 让 fmtguard 自动把 scope 扩到"整文件干净" | 违背"范围由调用方决定"的核心契约，等于把裁决权交回 formatter |
| 用 `RUSTC_BOOTSTRAP=1` 打开 `--file-lines` | 不可移植，设计文档已拒 |
| 提高默认 `--engine-timeout-secs` 来"修"超时 | P0-1 是管道自锁死，调大只是把 8 s 拖成 8 min（cankey 120 s×2 已实测） |
| 让 `log prune` 默认真删 | 台账是事实源/取证材料（cankey 设 7 天证据窗口）⇒ 默认 dry-run，只归档 |
| 给 0.2.0 旧事件日志做字段回填迁移 | 现有"缺字段按 0/null 兼容"已足够；重写历史会破坏 replay 字节一致 |

---

## 8. 跨仓影响（做之前先看）

- **P3-self（已落地）会让别人的负向控制失效**：`dsfolder/RUST-REPO-LOS-GOVERNANCE-DESIGN-2026-10-07.md` 把"真实红门禁 `cargo fmt --check`（fmtguard）"当成失败闭环的**负向控制臂**。自己的 fmt 一旦清零，这一臂会静默变绿 ⇒ 必须同时把该控制改指到别的必红门禁，并更新 los 侧 baseline。
- **P0-1 修好后 cankey/grok-build/gpui-kit/hotpath-rs 的"大文件不可用"备注要一起撤销**（cankey`docs` 里 25 份提到、18 份写着超时，其中 4 处明确"另案未修"）。
- **P1f-1 版本升级**：升级二进制会改变所有消费仓的台账字段形状（新增 `debt`/`ts`/`run_error`）；消费方（canpad 的 CI、los 治理 runner、cankey 的清理窗口）按"新字段可缺省"处理。

## 9. 排期建议

1. **P0-1**（唯一"工具不可用"级问题，改动小、证据硬）→ ✅ **v0.2.9 已修**（含 `clip_ms` 归因），门禁双臂 + 负向控制已落地；
2. **P1f-1 doctor/版本门禁 + P3-payload + P3-self + P3-hygiene**（都是小改动，且互不依赖，先清"装的是旧版/载荷不干净/自己不符合自己的规矩"）；
3. **P1e 台账四项**（失败留证 + 保留策略 + 损坏归档）→ ✅ **v0.3.0 已随 P1f-1 一批落地**（G14/G15/G16）；
4. **P1d 债语义**（改门禁语义，需要两臂控制与文档同步）；
5. **P1f-2/3/4**、**P2-jj**、**P2-plugin**、**P3-release**。

## 10. 复跑命令（附录）

```sh
# 台账汇总（9 份日志）
cd ~/syncfolder/project && python3 - <<'PY'
import json,glob,collections
paths=sorted(glob.glob("*/.fmtguard/runs.jsonl"))+sorted(glob.glob("*/*/.fmtguard/runs.jsonl"))
c=collections.Counter(); runs=collections.Counter(); g=collections.Counter()
for p in paths:
    for l in open(p):
        o=json.loads(l); t=o.get('t')
        if t=='run_start': runs[o['version']]+=1
        if t=='report_emit': c[o['verdict']]+=1
        if t=='gate_check' and not o['pass']: g[o['gate']]+=1
print(len(paths),'ledgers',dict(runs),dict(c),dict(g))
PY

# P0-1 复现（170 KB / 4000 行）
cd /tmp/fg-repro && time rustfmt --emit stdout --edition 2021 big.rs >/dev/null
time ~/.cargo/bin/fmtguard --changeset cs.json --engine-timeout-secs 8 --emit json

# P1d 基线债夹具
cd /tmp/fg-debt && ~/syncfolder/project/dsfolder/fmtguard/target/debug/fmtguard \
  --changeset cs.json --verify-fmt-check --emit json    # 期望 rejected（基线债误伤）

# 存量债（只读）
for d in cankey canpad cantool hotpath-rs gpui-kit qingjian terminal-browser grok-build; do
  echo -n "$d: "; (cd ~/syncfolder/project/$d && cargo fmt --all --check 2>&1 | grep -c '^Diff in'); done

# 载荷卫生
cd ~/syncfolder/project/dsfolder/fmtguard && cargo package --list --allow-dirty | grep -E 'rustopt|ITERATION'
```
