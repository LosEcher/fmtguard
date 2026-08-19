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
6. **不是 git/jj 仓库 → exit 2**，绝不静默全量格式化。
7. **事件日志是事实源**：报告/patch 都可以从 `.fmtguard/runs.jsonl` 重建；审计问"这个
   文件是谁格式化的"→ `jq 'select(.t=="fmt_result" and .file=="src/x.rs")' .fmtguard/runs.jsonl`。

## 8. 建议注入系统提示的片段

> 修改 Rust 代码后，用 `fmtguard --scope-from-git --emit patch` 检查待应用格式改动，
> 确认 patch 只覆盖你实际编辑过的文件/区域后再 `--apply`。任何 `exit 1`（门禁拒绝）都
> 表示格式化越界或超预算：先检查你的改动，不要盲目放宽预算。`exit 2` 表示 fmtguard
> 自身出错（含 rustfmt 解析失败），修好源文件再重试。绝不用 `cargo fmt` 替代 fmtguard——
> 后者会重排整个 workspace。

## 9. 新增能力（v0.2.0，P1a）

- **幂等门禁**：formatter 输出二次格式化必须无变化，否则 `engine.idempotent` 拒绝。
  工具级表现：`--apply` 一次后再运行，第二次必然 nothing to do。
- **`fmtguard replay <runId> [--emit json|patch] [--log <path>]`**：从事件日志重建
  该次运行的报告/patch，与原始输出字节一致。**审计专用，刻意不做 replay --apply**——
  存储的 patch 描述的是运行时的文件状态，事后盲目重放可能损坏已变更的文件（fail-closed）。
  runId 从 `--emit json` 输出的 `run_id` 字段获取。
- **`--apply --sandbox`（仅 git）**：先把「agent 改动 + 格式化 patch」同步进隔离的
  `git worktree`，在 worktree 内跑 `git diff --check` 验证，通过后才写主树；
  无论成败 worktree 都会清理。jj 仓库显式不支持（exit 2，jj worktree 与 change
  绑定需单独设计）。`--sandbox` 不带 `--apply` 会直接报错。

## 10. 路线图（当前状态）

- **已发布（P0）**：git/jj 变更检测、E3 引擎（stable rustfmt + hunk 裁剪）、5 道机械门禁、
  事件溯源日志、`--emit json|patch`、`--apply` fail-closed、exit 0/1/2 契约。
- **已发布（P1a / v0.2.0）**：引擎级幂等门禁、`fmtguard replay`、`--sandbox` worktree 隔离。
- **P1b（计划）**：E1 rust-analyzer rangeFormatting、E2 file-lines（以基准实测决定默认引擎）。
- **P2（计划）**：cargo check 门禁（接 `--sandbox`）、DSH 插件包装（`rust_fmt_changes` 工具）。
