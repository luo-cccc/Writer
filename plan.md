# Writer 复用能力产品化计划

## 目标

将 Writer 从通用终端智能体彻底迁移为长篇小说创作工具。复用原项目成熟的上下文、工具、会话、子智能体、RLM、快照、路由和扩展能力，但重新抽象为小说工程能力，而不是把小说创作作为一个附属模块挂在原产品上。

核心原则：

- 小说是本地长期工程，不是单轮聊天。
- 记忆管理是产品核心，不是审查附属物。
- 审查能力必须转为连续性诊断，不能变成模板化创作流水线。
- 工具服务人物、事件、伏笔、知识边界、章节状态和正文质感。
- 写作流程允许被打断、跳过和重排，不能强迫用户按固定表格生产正文。

## 能力复用总览

| 原项目能力 | 可复用本质 | 小说产品抽象 | 应落地的增强 |
|---|---|---|---|
| CLI/TUI 双入口 | 稳定命令调度和交互运行时 | `deepseek` 打开小说工作台，顶层命令管理书稿 | 隐藏旧代码入口，主帮助只展示小说工作流 |
| 项目上下文加载 | 自动发现并注入工作区约束 | 小说工程上下文包 | 优先注入 `book.toml`、`bible/`、`cards/`、`outline/`、`chapters/`、`memory/graph.json` |
| 文件读写工具 | 可控的本地资产维护 | 书稿资产读写 | 对章节、人物卡、设定卡、记忆摘要提供安全写入约束 |
| project_map | 结构扫描和摘要 | 小说项目地图 | 展示卷、章、人物卡、地点、伏笔、记忆文件结构 |
| review 工具 | 独立模型诊断 | 长篇记忆诊断 | 检查连续性、知识边界、时间线、伏笔和候选记忆更新 |
| 子智能体 | 并行分工与隔离上下文 | 创作编辑部 | 设定管理员、人物弧线编辑、时间线编辑、伏笔编辑、章节起草者、连续性诊断员 |
| RLM | 大文本分片和递归分析 | 长篇手稿分析器 | 批量分析几十章，抽取人物线、事件线、伏笔线和风险 |
| compaction/relay | 长会话保活和接力 | 创作接力 | 保留当前卷、章节目标、人物状态、未回收伏笔和下一步写作压力 |
| session 管理 | 会话恢复和历史 | 创作现场恢复 | 按作品、卷、章节恢复上下文 |
| snapshot/diff/undo | 变更保护 | 草稿版本保护 | 修订前后对比、回滚用户原文、显示改动范围 |
| skills | 可插拔专业知识 | 写作技法包 | 类型文、人物塑造、悬疑伏笔、爽点节奏、对白、场景等技能 |
| MCP | 外部工具桥接 | 资料库/设定库接入 | 接本地素材库、百科、时间线工具、知识库 |
| 模型路由 | 按任务选择模型和推理强度 | 创作任务路由 | 大纲/诊断用强推理，正文/改写用更高创造温度 |
| 状态栏/Home | 运行态仪表盘 | 小说工作台仪表盘 | 当前章、草稿/终稿、记忆图、未处理风险、下一步动作 |

## 产品抽象

### 1. 小说工程模型

将工作区识别逻辑从“代码项目”迁移为“小说工程”。

标准目录：

```text
book.toml
bible/
  premise.md
  world.md
  reader_promise.md
  style.md
craft/
  human_texture.md
  anti_ai_patterns.md
cards/
  characters/
  world/
  locations/
outline/
  master_plan.md
  chapter_index.md
chapters/
  001/
    brief.md
    craft_plan.md
    draft.md
    audit.md
    final.md
memory/
  graph.json
  summaries/
  facts.jsonl
  events.jsonl
  foreshadowing.jsonl
exports/
```

产品含义：

- `book.toml` 是作品根身份。
- `bible/` 是不可轻易破坏的作品契约。
- `cards/` 是实体事实库。
- `outline/` 是结构意图，不是正文模板。
- `chapters/` 是章节产物区。
- `memory/` 是长篇连续性系统。

### 2. 长篇记忆图

将 code-review-graph 类能力抽象为叙事记忆图。

节点类型：

- `book`: 作品。
- `chapter`: 章节。
- `character`: 人物。
- `location`: 地点。
- `world_rule`: 世界规则。
- `event`: 事件。
- `object`: 物件或资源。
- `relationship`: 关系状态。
- `promise`: 读者承诺或伏笔。
- `secret`: 秘密。
- `knowledge`: 人物已知/未知的信息。

边类型：

- `APPEARS_IN`: 出现于。
- `KNOWS`: 人物知道。
- `DOES_NOT_KNOW`: 人物不知道。
- `OWNS`: 拥有。
- `WANTS`: 想要。
- `FEARS`: 恐惧。
- `CAUSES`: 导致。
- `CHANGES`: 改变状态。
- `PROMISES`: 埋下承诺。
- `PAYS_OFF`: 回收承诺。
- `CONFLICTS_WITH`: 与既有记忆冲突。
- `AFFECTS`: 修改会影响。

必须支持：

- `memory build`: 从资产重建图。
- `memory status`: 展示节点、边、高度连接实体。
- `memory context N`: 为第 N 章生成上下文包。
- `memory query X`: 查询人物/地点/事件邻域。
- `memory impact N`: 改写第 N 章前显示影响范围。
- `remember N`: 从章节正文提取候选记忆更新。

### 3. 连续性诊断，不是模板审查

原 review 能力只保留“发现风险”的本质。

诊断对象：

- 人物状态是否跳变。
- 人物是否知道了不该知道的信息。
- 时间线是否矛盾。
- 地点、物件、伤病、资源是否连续。
- 伏笔是否丢失、提前暴露或无代价回收。
- 新事实是否需要进入记忆图。
- 改写某章会影响后续哪些章节。

禁止行为：

- 不给正文打模板化分。
- 不要求每章都有同样结构。
- 不用“爽点/反转/钩子”表格压扁所有类型。
- 不把自然表达改成统一句式。

诊断输出应包含：

- `BLOCKER`: 会破坏故事事实或主要人物可信度。
- `MAJOR`: 会伤害连续性、伏笔或读者承诺。
- `MINOR`: 局部可修问题。
- `CANDIDATE_MEMORY_UPDATES`: 可写入记忆图的候选项。
- `AFFECTED_NODES`: 受影响人物、地点、事件、伏笔和章节。

### 4. 人味正文赋能

原项目的模型调用、上下文注入、技能、子代理和记忆系统应服务正文质感。

正文生成时必须读取：

- 作品契约。
- 当前章节简报。
- 相关人物卡。
- 记忆图上下文包。
- 近期章节摘要。
- 前几章正文片段。
- 可选 craft 札记。

正文生成要强调：

- 人物会误判、遮掩、犹豫、抢话、回避痛点。
- 选择要留下后果。
- 情绪靠动作、停顿、物件处理、对白节奏外化。
- 场景推进靠冲突和代价，不靠旁白总结。
- 保留自然中文的长短句变化、停顿和留白。

反 AI 腔规则：

- 避免万能转折词堆叠。
- 避免段末总结升华。
- 避免人物把设定讲成说明书。
- 避免所有人物说话同一种语气。
- 避免每个场景都按同一节奏结束。

## 原能力改造计划

### 阶段一：主入口产品化

目标：用户打开就是小说工具，不是通用 TUI 加小说命令。

任务：

- 顶层 CLI 命令只展示小说主路径。
- `deepseek` 默认进入 Novel Studio。
- `deepseek init` 创建小说工程。
- `deepseek status` 展示小说工作区状态。
- `deepseek memory ...` 管理长篇记忆图。
- 旧 `novel/review/apply/pr` 入口隐藏，仅保留兼容。

验收：

- `deepseek --help` 不显眼展示代码审查入口。
- 初始化后立即存在 `book.toml` 和 `memory/graph.json`。
- `status` 输出章节、草稿、终稿、记忆图状态。

### 阶段二：TUI 工作台产品化

目标：交互界面内也以小说工程为主路径。

任务：

- `/home` 显示小说仪表盘。
- `/status` 显示小说工程状态。
- `/memory` 默认显示书稿记忆图，而不是个人偏好记忆。
- 新增 `/plan`、`/brief`、`/empower`、`/write`、`/audit`、`/revise`、`/remember`。
- `/review` 从命令面板移除，兼容路径提示改用 `/audit`。

验收：

- 命令面板优先出现小说命令。
- `/memory context N` 可直接给出第 N 章上下文包。
- `/write N` 生成的是章节写作任务指令，不是代码任务。

### 阶段三：项目上下文包升级

目标：模型自动看到最关键的小说资产。

任务：

- `ProjectContextPack` 增加 `novel` 字段。
- 提升 `book.toml`、`bible/`、`cards/`、`outline/`、`memory/graph.json` 权重。
- 最近章节摘要优先于整本正文。
- 章节目录按编号排序。
- 图谱只给统计和高价值摘要，避免上下文爆炸。

验收：

- 没有 `book.toml` 时仍可作为普通目录运行。
- 有 `book.toml` 时上下文包显示小说资产。
- 大量章节存在时不会一次性塞满上下文。

### 阶段四：记忆图结构化

目标：从“文件摘要图”升级为“叙事语义图”。

任务：

- 为 `memory/graph.json` 定义稳定 schema。
- `remember N` 输出结构化候选项。
- 支持人物知识边界。
- 支持伏笔生命周期：新埋、推进、回收、悬置、废弃。
- 支持事件时间线。
- 支持关系状态变化。
- 支持章节改写影响分析。

验收：

- 查询人物能看到出场章节、关系、秘密、当前状态。
- 查询伏笔能看到首次出现、推进、回收状态。
- 改写某章前能列出后续影响节点。

### 阶段五：创作编辑部子智能体

目标：复用子智能体并行能力，服务长篇协作。

角色：

- `explore`: 资产侦察，快速读项目结构。
- `plan`: 故事规划，做卷纲、人物弧线、结构方案。
- `review`: 记忆诊断，只读连续性风险。
- `implementer`: 起草或修订章节资产。
- `verifier`: 连续性验证，检查记忆更新是否完整。
- `custom`: 用户自定义写作角色。

任务：

- 改写角色说明和路由提示。
- 默认任务示例改为小说任务。
- 输出格式保留 `SUMMARY/EVIDENCE/CHANGES/RISKS/BLOCKERS`，但语义改为书稿资产。

验收：

- 子智能体不会自称代码审查员。
- `review` 角色不会要求模板化改正文。
- `implementer` 写文件时列出书稿资产路径。

### 阶段六：RLM 长篇手稿分析

目标：复用大文件和递归分析能力处理长篇正文。

使用场景：

- 批量读取 50 章。
- 抽取人物出场轨迹。
- 检查时间线矛盾。
- 检查伏笔是否丢失。
- 分析某人物说话方式是否漂移。
- 找出“解释过量”和“AI 腔”高风险段落。

任务：

- 为 RLM 增加小说分析提示模板。
- 支持按章节、人物、地点、事件分片。
- 输出可写入 `memory/` 的结构化结果。

验收：

- 对大量章节不会把全文塞入父上下文。
- 分析结果能回写成记忆摘要或诊断报告。

### 阶段七：版本保护和修订体验

目标：保护用户原文，让改写可控。

任务：

- 修订前自动记录章节快照。
- `diff` 展示草稿和终稿差异。
- `undo` 可回滚最近章节改动。
- 大范围改写前明确列出将改变的文件。
- 对用户原文默认保守，只有明确请求才覆盖。

验收：

- `/revise N` 不会无提示删除草稿。
- 修改后能看到改动文件和验证结果。
- 用户可恢复被改坏的章节。

### 阶段八：技能系统写作化

目标：把 skills 变成写作技法和类型知识包。

内置技能建议：

- `character-arc`: 人物弧线。
- `dialogue`: 对白和潜台词。
- `suspense`: 悬疑伏笔。
- `webnovel-pacing`: 网文节奏。
- `scene-pressure`: 场景压力。
- `human-texture`: 人味正文。
- `anti-ai-prose`: 反 AI 腔。
- `worldbuilding`: 世界观一致性。

验收：

- 技能说明面向写作。
- 技能不会覆盖用户设定。
- 技能输出可进入 brief、craft_plan 或 audit，而不是强制正文模板。

### 阶段九：外部资料和 MCP

目标：保留扩展性，服务资料型创作。

可接入：

- 本地素材库。
- 世界观资料库。
- 历史/地理/技术资料。
- 时间线工具。
- 人物关系图工具。
- 读者反馈库。

约束：

- 外部资料是素材，不是系统指令。
- 不自动引入第三方服务。
- 不把外部文本直接覆盖作品设定。

验收：

- 资料可被引用到章节简报或设定卡。
- 引用来源清晰。
- 不破坏既有 canon。

### 阶段十：质量报告闸门化

目标：把质量报告从“提示参考”升级为可执行的创作流水线闸门，但不把正文变成模板。

任务：

- `ContextQualityReport` 在 `/write` 前明确列出缺失资产、缺失人物卡、缺失伏笔、缺失摘要和题材专属上下文缺口。
- `ChapterQualityReport` 在 `/audit` 前提供确定性质量信号，并按题材隔离专项指标，避免玄幻规则污染其他题材。
- 为报告结果定义风险等级：`blocker`、`major`、`minor`、`signal_only`。
- 当 blocker 级上下文缺口存在时，提示用户先补 brief/card/memory，而不是直接幻想补全。
- `/revise` 只读取 Top 3 可行动问题，禁止把报告当作全文重写许可。

验收：

- 非玄幻项目不输出 `xianxia_*` 修订目标。
- 玄幻仙侠项目能识别资源无锚点、战斗无知见循环、对白声口重复、世界观未行动化等问题。
- `/write`、`/audit`、`/revise` 的报告字段稳定，可被测试固定。

### 阶段十一：玄幻仙侠资源经济系统

目标：把玄幻仙侠从“文风提示”推进到“资源、境界、宗门、法宝、代价”的可追踪系统。

新增目录建议：

```text
cards/resources/
  spirit_stone.yaml
  foundation_pill.yaml
  flying_sword.yaml
  sect_contribution.yaml
```

资源卡字段：

```yaml
id:
name:
category: currency|pill|artifact|manual|territory|favor
rarity:
market_value:
ordinary_income_equivalent:
who_controls_it:
cost_to_use:
debt_or_obligation:
first_seen:
last_changed:
canon_status:
evidence:
```

任务：

- 初始化玄幻/仙侠项目时提供 `cards/resources/_template.yaml`。
- `memory/graph.json` 支持资源节点和资源归属变化。
- `ContextQualityReport` 检查关键资源是否有价值、稀缺性、代价或控制方。
- `ChapterQualityReport` 检查资源获得是否缺少价格、收入对照、债务、宗门义务或身体/规则代价。
- `/remember` 能提取资源获得、资源消耗、法宝归属、功法限制等候选记忆更新。

验收：

- 主角获得重要丹药、法宝、灵石、功法时，系统能提示缺少代价或来源。
- 改写章节前能显示资源变动影响哪些人物、宗门、伏笔和后续章节。
- 资源卡不会被外部素材直接覆盖，必须走候选记忆或用户确认。

### 阶段十二：审查报告分层

目标：避免 `/audit` 一个报告承担所有问题，导致连续性、文风、记忆候选和读者承诺混在一起。

报告分层：

- `ContinuityAudit`: 人物状态、时间线、知识边界、地点状态、物件归属、canon 冲突。
- `CraftAudit`: 人味、对白、节奏、战斗、世界观入戏、AI 腔。
- `MemoryCandidateAudit`: 只负责抽取可入库事实和候选更新。
- `ReaderPromiseAudit`: 本章承诺、追读压力、伏笔推进、回收和悬置。

任务：

- `/audit N` 输出总览，并在内部按四类报告组织发现。
- `/revise N` 的 Top 3 目标必须带来源类别，优先级高于单纯文本美化。
- `memory candidates` 只从 `MemoryCandidateAudit` 或明确候选区提取，避免误抓审稿正文。
- TUI 状态面板按类别展示未解决风险。

验收：

- 伏笔断裂不会和“短句不足”在同一优先级里混排。
- 没有候选记忆时明确输出 `none`，不会从普通 BLOCKER 文本误抽候选。
- `/revise` 能说明本次修的是连续性、技法、记忆候选还是读者承诺问题。

### 阶段十三：失败样本库与评估基础设施

目标：先建设评估基础设施和失败样本归档能力，但不在功能开发未完成前判定真实写作上限。

目录：

```text
eval/
  failures/
    knowledge_leak/
    promise_drift/
    fake_emotion/
    resource_without_cost/
    combat_power_spam/
    revise_overwrite_voice/
  fixtures/
  reports/
  rubrics/
```

任务：

- 为每类失败样本定义最小 fixture：原文片段、期望报告信号、期望修订方向。
- 增加 `deepseek eval collect-failure` 或等价内部命令，把失败章节归档成可回归样本。
- 建立 rubrics：连续性、人物声口、知识边界、伏笔状态、资源经济、修订保真。
- 所有新增报告信号必须至少有一个失败样本和一个非触发样本。

验收：

- 每个质量信号都能说明“它解决哪类真实失败”。
- 失败样本不依赖外部版权原文，使用原创或用户本地授权内容。
- 评估样本只用于回归质量信号，不宣称真实读者效果。

### 阶段十四：长跑实验脚手架

目标：为开发完成后的真实验证准备实验框架；本阶段只做工具，不提前宣称百万字能力。

新增目录：

```text
experiments/
  configs/
  runs/
  reports/
leaderboard/
```

任务：

- 记录每次写作实验的模型、温度、技能包、题材、目标字数、memory 配置和命令链。
- 支持连续生成 10/20/50/100 章的批处理脚手架。
- 自动收集每章 `ContextQualityReport`、`ChapterQualityReport`、`audit.md`、`memory candidates`、revision diff。
- 输出崩坏点候选：人物漂移、伏笔断裂、资源无代价、知识泄漏、修订洗稿、上下文膨胀。
- 真实长跑只在前面开发阶段完成后执行。

验收：

- 能复现实验配置。
- 能对比不同流程：无 memory、有 memory、有 archive、有 targeted revise、有题材技能包。
- 能输出结构化报告，但不把一次实验结果包装成最终能力结论。

## 命令体系目标

主命令：

```bash
deepseek init
deepseek status
deepseek plan
deepseek brief 1
deepseek empower 1
deepseek write 1
deepseek audit 1
deepseek revise 1
deepseek remember 1
deepseek memory build
deepseek memory status
deepseek memory context 12
deepseek memory query 林墨
deepseek memory impact 12
deepseek export
```

TUI slash 命令：

```text
/home
/status
/memory
/memory build
/memory context 12
/memory query 林墨
/memory impact 12
/plan
/brief 1
/empower 1
/write 1
/audit 1
/revise 1
/remember 1
```

兼容但不主动推荐：

```text
deepseek novel ...
deepseek review
deepseek apply
deepseek pr
/review
```

## 数据结构计划

### book.toml

```toml
title = "作品名"
genre = "题材"
language = "zh-CN"
target_words = 800000
current_volume = 1
current_chapter = 0
```

### memory/graph.json

```json
{
  "schema_version": 2,
  "updated_at": "2026-05-13T00:00:00Z",
  "nodes": [
    {
      "id": "character:lin_mo",
      "kind": "character",
      "label": "林墨",
      "source": "cards/characters/lin_mo.yaml",
      "summary": "主角，当前隐瞒重生记忆",
      "state": {
        "last_seen_chapter": 12,
        "knowledge": ["知道陈岚说谎"],
        "unknown": ["不知道父亲仍活着"]
      },
      "hash": "..."
    }
  ],
  "edges": [
    {
      "kind": "KNOWS",
      "source": "character:lin_mo",
      "target": "secret:chen_lan_lie",
      "evidence": "chapters/012/final.md:88",
      "confidence": 0.91
    }
  ]
}
```

### memory update candidate

```json
{
  "chapter": 12,
  "candidates": [
    {
      "kind": "character_state",
      "target": "林墨",
      "change": "确认陈岚在事故时间线上撒谎",
      "evidence": "chapters/012/final.md:88",
      "confidence": 0.92,
      "affects": ["character:chen_lan", "promise:accident_truth"]
    }
  ]
}
```

## 工程验证计划

本节只验证“功能是否按设计运行”，不验证“是否已经能稳定写出百万字级作品”。真实创作效果验证必须放到所有开发阶段完成之后，见后文“真实验证计划”。

基础验证：

- `cargo fmt --all`
- `cargo check -p deepseek-tui`
- `cargo check -p deepseek-tui-cli`
- `cargo test -p deepseek-tui novel`
- `cargo test -p deepseek-tui commands::memory`
- `cargo test -p deepseek-tui commands::status`
- `cargo test -p deepseek-tui-cli root_help_surface_contains_expected_subcommands_and_globals`
- `cargo test -p deepseek-tui-cli subcommand_help_surfaces_are_stable`

产品烟测：

```powershell
$tmp = Join-Path $env:TEMP ('ds-novel-product-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
cargo run -q -p deepseek-tui -- --workspace $tmp init --title 产品化烟测 --genre 悬疑 --premise 失踪者留下反常记忆
cargo run -q -p deepseek-tui -- --workspace $tmp status
cargo run -q -p deepseek-tui -- --workspace $tmp memory status
cargo run -q -p deepseek-tui -- --help
```

验收标准：

- 主帮助展示小说主路径。
- 初始化项目包含完整书稿目录。
- 记忆图能构建、查询、给上下文包。
- `/memory` 默认指向书稿记忆图。
- `/audit` 和 review 工具不输出模板化创作评分。
- 子智能体角色不再默认面向代码任务。

## 真实验证计划（所有开发完成之后执行）

真实验证的目标不是证明代码会跑，而是证明系统能在长篇创作中降低崩坏率。该阶段必须在阶段一到阶段十四完成、工程验证通过、失败样本库和实验脚手架可用之后执行。

### 验证顺序

1. **10 章短跑**
   - 目的：验证命令链、记忆候选、章节修订和报告闸门不会互相打架。
   - 产物：10 章 draft/final、10 份 audit、memory graph、regression report、失败样本归档。
   - 不做结论：不能据此宣称百万字能力。

2. **20 章连续性回归**
   - 目的：检查人物状态、伏笔状态、资源变动和知识边界是否仍可追踪。
   - 必跑：`/memory regression 10`、`/memory promises`、`/memory impact` 抽样。
   - 记录：每章人工修订时间、Top 3 修复命中率、新引入问题数。

3. **50 章长跑**
   - 目的：验证归档、上下文包、memory graph 增长和阶段摘要是否能控制上下文膨胀。
   - 必跑：`/memory archive 1 25`、`/memory regression 20`、RLM 批量连续性分析。
   - 记录：`memory/graph.json` 大小、上下文包 token 估计、候选记忆数量、未解决伏笔数量。

4. **100 章压力测试**
   - 目的：接近真实长篇生产压力，评估人物声口漂移、设定代价、伏笔回收和修订保真。
   - 对比组：无 memory、有 memory、有 archive、有 targeted revise、有玄幻/题材技能包。
   - 结论标准：只能基于数据说明哪个流程更稳，不能只凭单章观感下结论。

5. **读者/人工评审**
   - 目的：验证作品效果，而不是工具自嗨。
   - 样本：随机抽取原稿、修订稿、不同流程稿件。
   - 指标：可读性、人物可信度、追读意愿、设定真实感、情绪有效性、是否模板化。

### 真实验证指标

- 连续性硬伤数 / 章。
- 人物知识边界泄漏数。
- 伏笔新埋、推进、回收、悬置数量。
- 资源获得但无代价/来源/控制方的次数。
- `/audit` 命中后 `/revise` 成功修复比例。
- `/revise` 新引入问题比例。
- 每章平均人工修订时间。
- `memory/graph.json` 和上下文包增长曲线。
- 读者追读意愿评分。

### 禁止结论

- 禁止在 10/20 章实验后宣称“支持百万字”。
- 禁止用测试通过替代作品质量结论。
- 禁止只展示成功章节，不记录失败样本。
- 禁止把 LLM 自评当作唯一质量证明。

## 风险和边界

### 风险 1：把创作流程做成工厂流水线

缓解：

- `brief`、`empower`、`audit` 都是可选辅助。
- `craft_plan.md` 明确不是评分表。
- `/write` 可直接从记忆和大纲起草，不强制先 audit。

### 风险 2：记忆图过度结构化导致正文僵硬

缓解：

- 记忆图只约束事实和连续性。
- 正文生成仍以场景、人物选择、冲突和语言节奏为核心。
- 允许正文偏离 craft 札记，只要不破坏 canon。

### 风险 3：旧代码工具心智残留

缓解：

- 旧入口隐藏。
- 模型可见描述改为小说语义。
- project/context/review/subagent 全部改写为书稿资产语义。

### 风险 4：长篇上下文过载

缓解：

- 优先记忆摘要和图谱邻域。
- 近期章节限量读取。
- 大批量分析交给 RLM。
- relay 记录当前创作现场。

## 下一步优先级

原则：先补齐产品能力，再建设评估基础设施，最后做真实长跑验证。真实验证不得提前插队成为开发阻塞项，但所有开发都必须为最终真实验证保留数据出口。

1. 完成阶段十：质量报告闸门化。
2. 完成阶段十一：玄幻仙侠资源经济系统。
3. 完成阶段十二：审查报告分层。
4. 完成阶段十三：失败样本库与评估基础设施。
5. 完成阶段十四：长跑实验脚手架。
6. 回归阶段一到九的遗留项，确保主入口、TUI、memory、RLM、skills、MCP 都稳定。
7. 执行“工程验证计划”。
8. 在全部开发完成后，执行“真实验证计划”。

## 成功标准

这个项目完成产品级迁移后，用户不需要理解它原来是 TUI、代码代理或审查工具。用户看到的是：

- 一个可以初始化和维护长篇小说工程的工作台。
- 一个能记住几十万字设定、人物、伏笔和状态变化的记忆系统。
- 一个能像编辑部一样拆分任务的创作代理系统。
- 一个能保护原文、诊断连续性、辅助修订的写作环境。
- 一个能写出更自然、更有人味正文的长期创作工具。
