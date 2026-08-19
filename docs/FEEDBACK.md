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

## 迭代观察清单（供 P1b/P2 排期）

- [ ] crates.io 发布（`cargo publish`，publish-ready 已验证）
- [ ] 补 DESIGN.md：整理被拒替代方案清单（cargo fmt 全量 / --file-lines / tree-sitter / daemon / 散文式 CI 规则）
- [ ] 全局配置文件支持（目前配置全走 CLI flag；跨项目统一预算需重复传参）
- [ ] DSH 插件包装 `rust_fmt_changes` 工具（路线图 P2，已有变更集协议的 agent 入口）
