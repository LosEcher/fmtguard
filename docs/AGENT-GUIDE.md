# fmtguard — AI Agent 使用指引

本文件面向**编码 agent**（Codex / Claude Code / Cursor / DSH 类 harness）与自动化脚本。
核心心智模型只有一句话：

> **范围由调用方决定，fmtguard 只负责执行与验证。范围没声明清楚，它宁可拒绝（fail-closed），也绝不替你猜。**

## 1. 什么时候用

- 你对一个 Rust 仓库做了**增量修改**（改了几个函数/文件），提交前想格式化自己动过的地方；
- 你只格式化**自己改过的行/文件**，绝不希望 `cargo fmt` 那样重排整个 workspace、generated 代码或 vendor；
- 你想让"格式化是否越界/超预算"变成**机械判定**（exit code），而不是靠人肉 diff review。

## 2. 快速参考

| 场景 | 命令 |
|------|------|
| 格式化工作树里被改过的 `.rs`（git 仓库，默认） | `fmtguard --scope-from-git` |
| 同上，jj 仓库 | `fmtguard --scope-from-jj` |
| 显式声明范围（推荐，见 §4） | `fmtguard --changeset changeset.json` |
| 只要 patch（配合 review / git apply） | `fmtguard --scope-from-git --emit patch` |
| 机器可读报告 | `fmtguard --scope-from-git --emit json` |
| 门禁全过后真正写盘 | `fmtguard --scope-from-git --apply` |
| 收紧预算（CI 常用） | `fmtguard --scope-from-git --budget-max-added-lines 50 --budget-max-ratio 1.5` |
| 只看"这次改动有没有新增格式债" | `fmtguard --scope-from-git --verify-fmt-check --emit json`（默认 delta） |
| 把范围外格式债与你的改动隔开 | `fmtguard --scope-from-git --hunk-context 0 --emit patch` |
| 看配置从哪来 | `fmtguard --dump-config`（仓内 `.fmtguard.toml` ∪ 用户配置 ∪ CLI） |
| 环境自检（版本/引擎/日志） | `fmtguard doctor --emit json` |
| 版本门上锁（防装旧版） | `fmtguard doctor --require-version 0.3.0` 或任意运行加 `--require-version 0.3.0` |
| 台账保留（先看再删） | `fmtguard log prune --keep-runs 200 --emit json`（`--apply` 才动盘，只归档不删） |

- **默认 dry-run**：不写盘。只有 `--apply` 才写，且只有**所有门禁通过**才会写。
- 事件日志默认落在 `<repo>/.fmtguard/runs.jsonl`（追加式 JSONL）；CI 里建议 `--log` 到 artifact 路径，失败时可回放。

## 3. 推荐工作流（提交前）

```sh
# 1. 你（agent）改完了若干 .rs 文件

# 2. 先看 fmtguard 打算改什么（dry-run，永不写盘）
fmtguard --scope-from-git --emit patch

# 3. 如果 patch 里出现了你没动过的文件/区域 —— 停下，检查你的 scope 声明，
#    不要直接 --apply。（正常情况 patch 只含你的改动区域 ± 3 行上下文）

# 4. 确认无误再应用
fmtguard --scope-from-git --apply

# 5. 检查结果并提交
git diff --stat && git diff --check
```

**提交前的机械检查清单（agent 必须逐条执行）**：

- [ ] `fmtguard --scope-from-git --emit json` 的 `verdict` 是 `ok`；
- [ ] `git status` 里被格式化改动的文件集合 ⊆ 你实际编辑过的文件集合；
- [ ] `git diff --stat` 的规模符合预期（没有被 formatter 放大 10 倍以上）；
- [ ] `git diff --check` 无空白错误；
- [ ] 有疑问时回放事件日志：`jq -c 'select(.t=="fmt_result")' .fmtguard/runs.jsonl | tail`。

## 4. Changeset 协议（agent 首选入口）

如果你知道**自己改动了哪些文件、哪些行**（多数 agent 都该知道——你刚写过它们），
用 `--changeset` 显式声明，fmtguard 完全按你的声明裁剪：

```json
{
  "base_ref": "HEAD",
  "files": [
    {
      "path": "src/router.rs",
      "ranges": [{ "start": 120, "end": 180, "reason": "added_handler" }],
      "agent_added_lines": 30
    }
  ]
}
```

要点：

- `ranges` 是**工作树里 1-based 含端点**的行区间（`start..=end`）。省略 `ranges` = 格式化整个文件。
- `agent_added_lines`（你自己新增的行数）喂 **diff-ratio 门禁**（默认 formatter 新增 ≤ 你新增 × 3）。
  不填则该文件跳过 ratio 门禁，但仍受 per-file added-lines 上限约束。
- **hunk 合并规则**：两个改动点相距 ≤ 6 行时会被并成一个 hunk 一起保留。
  所以"范围外还有 2 行格式错误"在边界附近（±3 行 context 内）是**预期行为**，不是 bug；
  要精确隔离，让范围外改动离范围边界 ≥ 7 行。
- `reason` 只是给审计日志看的备注，不影响行为。

## 5. 读报告（--emit json）

```json
{
  "verdict": "ok" | "rejected",
  "mode": "dry-run" | "apply",
  "stats": { "files_scanned": 1, "files_changed": 1, "added_lines": 2, "removed_lines": 2 },
  "gates":  [ { "gate": "budget.per_file_added", "pass": true, "metric": 2, "limit": 200 } ],
  "rejections": [ ],
  "patch": "diff --git ..."
}
```

- `verdict == "ok"`：可以 `--apply`。
- `verdict == "rejected"`：**没有任何文件被写盘**。读 `rejections` 数组，逐条看
  `gate` / `metric` / `limit`，判断是「formatter 真的越界了」（该收紧你的改动）还是
  「预算太紧」（该显式提高预算，见 §6）。
- `patch` 只在 `files_changed > 0` 时出现；`--emit patch` 时它就是 stdout 内容。

## 6. 预算与门禁（什么时候调、怎么调）

| gate | 默认 | 触发时先问自己 |
|------|------|----------------|
| `scope.containment` | — | formatter 改了 scope 外的文件？**这是 bug 信号，先查再放宽** |
| `budget.per_file_added` | 200 行/文件 | 你的改动本来就会让 rustfmt 重排大片？是 → 提高 |
| `budget.diff_ratio` | 3.0 | formatter 新增是你新增的 3 倍以上？是 → 检查改动范围，或提高 |
| `budget.max_files` | 5 | 你一次改了 >5 个文件？是 → 提高 |
| （拒绝详情里的 hunk 合并提示） | — | detail 说"kept hunk spans N lines although the scope declares M" ⇒ 范围外债离你的改动 < 2×context，**先试 `--hunk-context 0`**，不要直接放宽预算 |
| `whitespace.clean` | — | formatter 自己产生了尾随空白？**异常，先查** |

原则：**先怀疑自己，再放宽预算**。门禁存在的意义就是让"格式化扩大 diff"变成显式失败。

```sh
fmtguard --scope-from-git --budget-max-added-lines 500 --budget-max-ratio 5.0
```

## 7. 已知边界与坑（agent 必读）

1. **rustfmt 解析失败 → 整个 run 失败（exit 2），不写任何文件**。这是故意的（fail-closed）。
   你改出了语法错误时，先修语法再格式化。
2. **非 UTF-8 文件 → 该文件报错，run 失败**。Rust 源码应当 UTF-8。
3. **exclude 默认规则**：`generated/**`、`vendor/**`、`target/**`、`node_modules/**`（支持 `**`）。
   需要排除其他目录：`--exclude 'src/legacy/**'`。
4. **rustfmt 配置**：fmtguard 自动探测仓库根 `rustfmt.toml` / `.rustfmt.toml` 与最近的
   `Cargo.toml` edition；unstable 配置项 rustfmt 会忽略（带警告），不影响输出正确性。
5. **纯删除的文件**（diff 无新增行）：没有可格式化的范围，自动跳过。
   **新建但未 `git add` 的文件同理看不见**：`--scope-from-git` 只看 tracked diff，fmtguard 会打一行
   warning 并在 `scope.untracked` 里列出它们；`--include-untracked` 才把它们按整文件纳入范围。
6. **不是 git/jj 仓库 → exit 2**，绝不静默全量格式化。
7. **事件日志是事实源**：报告/patch 都可以从 `.fmtguard/runs.jsonl` 重建；审计问"这个
   文件是谁格式化的"→ `jq 'select(.t=="fmt_result" and .file=="src/x.rs")' .fmtguard/runs.jsonl`。
   每条事件都带 `ts`（RFC3339 UTC 毫秒）；`exit 2` 的失败会写 `run_error` + `report_emit{verdict:"error"}`，
   所以"为什么失败"可以从台账里读出来，而不是只留在 stderr。
8. **`replay` 的三种 verdict**：`ok` / `rejected` / **`error`**（有 `run_error`）/ **`interrupted`**
   （run_start 之后没有 report_emit：崩溃、被杀、日志被截断）。**`interrupted` 不是 `ok`**——
   旧版本会把悬空 run 报成 ok，审计时别把"没记录"读成"成功"。
9. **台账损坏自愈**：如果日志最后一行是半截写入（进程在 append 中途死掉），下一次运行会把
   整份日志改名成 `runs.jsonl.corrupt-<ts>`（取证保留）并另起新日志；不会在半截行后面继续追加。
10. **保留策略**：`fmtguard log prune --keep-runs N` 或 `--older-than 7d`（二选一，缺省报错）；
    默认 dry-run，`--apply` 时被裁掉的 run 先追加进 `.fmtguard/archive/`，再用临时文件+rename 原子替换。
    没有 `ts` 的旧日志用 `--older-than` 会被拒绝（不猜），改用 `--keep-runs`。

## 8. 建议注入系统提示的片段

> 修改 Rust 代码后，用 `fmtguard --scope-from-git --emit patch` 检查待应用格式改动，
> 确认 patch 只覆盖你实际编辑过的文件/区域后再 `--apply`。任何 `exit 1`（门禁拒绝）都
> 表示格式化越界或超预算：先检查你的改动，不要盲目放宽预算。`exit 2` 表示 fmtguard
> 自身出错（含 rustfmt 解析失败），修好源文件再重试。绝不用 `cargo fmt` 替代 fmtguard——
> 后者会重排整个 workspace。

## 9. 新增能力（v0.2.8，P1a/P1c/P1b/P2）

- **幂等门禁**：formatter 输出二次格式化必须无变化，否则 `engine.idempotent` 拒绝。
  工具级表现：`--apply` 一次后再运行，第二次必然 nothing to do。
- **`fmtguard replay <runId> [--emit json|patch] [--log <path>]`**：从事件日志重建
  该次运行的报告/patch，与原始输出字节一致。**审计专用，刻意不做 replay --apply**——
  存储的 patch 描述的是运行时的文件状态，事后盲目重放可能损坏已变更的文件（fail-closed）。
  runId 从 `--emit json` 输出的 `run_id` 字段获取。
- **`--apply --sandbox`（git 与 jj 都支持）**：先把「agent 改动 + 格式化 patch」同步进隔离树，
  验证通过才写主树，无论成败都清理隔离树。
  git 用 `git worktree add --detach` + `git diff --check` + `cargo check`；
  jj 用 `jj workspace add`（工作副本 commit 落在当前 commit 的父上）+ `cargo check`
  （`jj diff` 没有 `--check`，空白仍由 `whitespace.clean` 门禁覆盖），
  收尾 `jj workspace forget` + `abandon` 那个孤儿 change，并断言 `jj workspace list` 里没有残留。
  `--sandbox` 不带 `--apply` 会直接报错；`--changeset`（无 VCS 上下文）不支持 sandbox。

## 10. 路线图（当前状态）

- **已发布（P0）**：git/jj 变更检测、E3 引擎（stable rustfmt + hunk 裁剪）、5 道机械门禁、
  事件溯源日志、`--emit json|patch`、`--apply` fail-closed、exit 0/1/2 契约。
- **已发布（P1a / v0.2.0）**：引擎级幂等门禁、`fmtguard replay`、`--sandbox` worktree 隔离。
- **v0.2.1**：超时诊断、rustfmt 耗时观测、RA LSP 协议底座；E1 仍需显式实验，不改变默认 E3。
- **P2（部分）**：`--verify-fmt-check` 对待写入的 scoped candidate 做整文件 clean 检查；scope 外已有格式债时显式 rejected。
- **P2（部分）**：JSON `files[]` 与 `fmt_result` 事件增加 `out_of_scope_hunks`，显示被裁剪掉的格式化 hunk 数量。
- **P1b（设计完成）**：E1 rust-analyzer rangeFormatting、E2 file-lines（以基准实测决定默认引擎）。协议与验收见 `docs/P1B-RANGE-ENGINE-DESIGN.md`；当前 stable rustfmt 明确不支持 `--file-lines`，在 RA LSP 会话落地前继续使用 E3。
- **P1c（新增反馈）**：大型文件超时可观测性（bytes/lines、阶段耗时、路径化错误）与大文件引擎选择；超时仍 fail-closed，不通过放宽预算静默掩盖。
- **v0.2.4（P1c）**：`fmt_result`、JSON 报告与 replay 拆分首轮 rustfmt 和幂等校验耗时，同时保留总耗时；旧日志缺失字段按 `0` 兼容。
- **v0.2.5（P2）**：`--apply --sandbox` 在隔离 worktree 先执行 `git diff --check`，再执行 `cargo check --quiet`；任一步失败均 fail-closed，不写主工作树。
- **v0.2.6（P1c）**：LSP 超时清理先终止 rust-analyzer 进程组，再终止直接子进程，减少孤儿辅助进程。
- **v0.2.7（P1b）**：新增 E1 RA rangeFormatting 结果适配层；保持实验性 API，默认 E3 与 CLI 行为不变。
- **v0.2.8（P1b）**：新增显式 `--engine e1|e3`；E1 使用短生命周期 rust-analyzer，E3 仍为默认。
- **v0.2.9（P0-1）**：修复 rustfmt 子进程管道自锁死——stdout/stderr 在 spawn 后立刻起排空线程，
  大文件不再被误报为「rustfmt 超时」（cankey 129 KB `engine.rs`：修复前 30.07 s 超时 → 修复后 0.18 s ok）。
  门禁新增 **G11**（≈258 KB 输入 + 单行 scope：<10 s、exit 0、只改范围内函数）与 **G12**
  （睡死 stub：exit 2、不写盘）。事件/报告/replay 新增 **`clip_ms`**：fmtguard 自己的 diff/clip 耗时，
  从 `rustfmt_duration_ms`（= 整个文件的 wall time）中分离出来；全文件重排时 `clip_ms` 才是大头
  （实测 4000 行：rustfmt 71+68 ms vs clip 8554 ms）。**未做**：裁剪 diff 的限幅（见 P1g）。
- **v0.3.1（P1d）**：`--verify-fmt-check` 分档——默认 **`delta`**（只拒绝**本次改动范围内**新增的格式债；
  范围外既有债只计数并如实上报），`--verify-fmt-check=strict` 保留"整文件必须是 rustfmt 不动点"的旧语义
  （在带债仓里会误伤：实测同一夹具 delta=ok / strict=rejected）。报告与事件新增
  `in_scope_debt_hunks` / `fmt_clean`，`stats.out_of_scope_hunks` 汇总被裁掉的 hunk 数；当它非零时
  stderr 打一行"scope-clipped formatter: N hunk(s) dropped … this verdict does NOT assert repo cleanliness"。
- **v0.3.0（P1f-1 + P1e）**：`fmtguard doctor`（read-only 环境自检：formatter 版本与**行为探针**、
  E1/E3 可用性、config/edition 发现、日志健康）+ 全局 `--require-version X.Y.Z`（不满足即 exit 2，
  用来把"装的是旧版"变成机械门禁）；台账生命周期：每事件 `ts`、失败写 `run_error`、
  `replay` 新增 `error`/`interrupted`（悬空 run 不再报 ok）、`log prune`（默认 dry-run、只归档不删）、
  半截日志自动隔离为 `runs.jsonl.corrupt-<ts>`。
- **v0.4.0（P1f-2/3/4）**：`--include-untracked` + untracked 警告（`scope.untracked`）；
  `--hunk-context N`（0–6，默认 3）把"改动内外合并"变成可控参数，预算拒绝的 detail 现在会说明
  hunk 合并（`kept_lines`/`scope_lines`、`min_cross_gap`，并给出 `--hunk-context 0` 的建议）；
  配置文件 `<repo>/.fmtguard.toml` 与 `~/.config/fmtguard/config.toml`（支持 TOML 子集：
  comments / `[section]` / 字符串 / 数字 / 布尔 / 字符串数组；未知键报错），`--dump-config` 打印
  每个键的来源（default / file:… / cli），`--no-config` 忽略文件。
- **v0.4.2（P1g）**：裁剪 diff 有自己的预算 `--diff-timeout-secs N`（默认 10，0 = 不限）。
  超预算时 **fail-closed exit 2**，错误信息点名"scope clipping hit its Ns budget（M changed line(s)）"，
  不再让 8 s 级的 clip 成本伪装成 rustfmt 超时、也不再无限期地等；实测全文件重排 4000 行：
  预算 1 s → **1.19 s exit 2**（旧行为 8.7 s 静默算完再走预算门禁），而"254 KB + 单行改动"
  （`clip_ms` 22）在同样 1 s 预算下照常 ok。报告/事件早在 v0.2.9 起就有 `clip_ms` 把这段成本单列。
- **v0.4.1（P2-jj）**：`--apply --sandbox` 支持 jj 仓库（`jj workspace add` + `cargo check` +
  `workspace forget`/`abandon` 清理，并断言无残留工作区）；git 路径不变。
- **v0.4.2 补（P3-self）**：本仓自己先用 fmtguard 清掉 41 处 rustfmt diff，并在 `ci.yml` 里把
  `cargo fmt --all --check` 变成门禁（负向控制实测必红）。
- **v0.4.2 补（P3-release / P3-docs）**：`.github/workflows/release.yml`（4 平台 `--profile dist`
  资产 `fmtguard-<os>-<arch>`、tag↔Cargo 断言、发布前对产物跑 `--version` + `doctor` 冒烟）；
  仓内新增 `docs/DESIGN.md`（分层、契约、决策与**拒绝清单**、已知限制），README 已链接。
- **v0.5 / P2-plugin（2026-10-07）**：DSH 插件 `dsh-fmtguard`（`dsplugins/dsh-fmtguard`）把
  `rust_fmt_changes`（+ `fmtguard_doctor`）暴露成 agent 工具：内联 changeset、预算/hunk-context/
  diff-timeout/verify-fmt-check 全参数化，exit 1 映射成 `verdict=rejected`（业务结果，不写盘），
  exit 2 映射成 `toolError`。二进制解析 `config.binary → $FMTGUARD_BIN → PATH → ~/.cargo/bin`。
  门禁：preflight（web+headless）、冒烟 6/6、host-smoke 2/2、headless E2E 真调用。

### 2026-10-07 跨项目巡检（立项，详见 [`docs/ITERATION-CANDIDATES-2026-10-07.md`](ITERATION-CANDIDATES-2026-10-07.md)）

- **P0-1（bug，✅ v0.2.9 已修）**：`rustfmt` 子进程的 stdout/stderr 原本直到 `try_wait` 返回状态才排空 ⇒ 格式化输出 ≥64 KiB 的管道阻塞被误报成「rustfmt 超时」（cankey 25 份文档提到、18 份写超时）。修复后真实病例 0.18 s 通过；门禁 G11/G12 双臂。
- **P1d（✅ v0.3.1）**：`verdict: ok` 只声明「范围内无新增债」——现在报告/事件带 `in_scope_debt_hunks`、`fmt_clean`、`stats.out_of_scope_hunks`，stderr 有明示行；`--verify-fmt-check` 分档默认 `delta` / `strict`（整文件语义在带债仓里会把**别人的债**判给当前调用方）。门禁 G17 四臂。
- **P1e（✅ v0.3.0 已修）**：台账生命周期——每事件 `ts`、失败写 `run_error` + `report_emit{error}`、`replay` 的 `error`/`interrupted`、`log prune`（默认 dry-run、归档不删）、半截日志隔离为 `runs.jsonl.corrupt-<ts>`；门禁 G14/G15/G16。
- **P1f（✅ v0.4.0 全部落地）**：`doctor` + `--require-version`（v0.3.0）、untracked 警告 + `--include-untracked`、`--hunk-context N` + 合并诊断、配置文件 + `--dump-config`（门禁 G13/G18/G19/G20）。
- **P2/P3**：jj 的 `--apply --sandbox`（✅ v0.4.1）、DSH 插件包装、release/publish workflow（v0.2.1–v0.2.8 的 GitHub release 均 0 asset）、自举格式化自身 + CI 加 `cargo fmt --all --check`（需同步改 los 侧把它当红的负向控制臂）、`.rustopt/` 不进 `.crate`、补 `docs/DESIGN.md`。
