# bnote 文档合规复查

- 日期：2026-10-21
- 基准：`GUIDELINES.md`（CORE 工程常量与声明规则已澄清）
- 范围：`bnote/README.html`、`bnote/USER.html`、`bnote/CORE.html`、`bnote/PROVIDER.html`、`bnote/BUILD.html`、`bnote/diagram/`、`bnote/reference/`
- 结论：**通过，无未关闭项。**

## 1. 文档集合与边界

| 文档 | 边界 | 当前内容 | 结果 |
|---|---|---|---|
| `README.html` | — | WHAT & WHY、用户交互、Credits | 通过 |
| `USER.html` | upper | TERMS、CONVENTIONS、DIAGRAMS、INTERFACES | 通过 |
| `CORE.html` | 内部 | 文件树、CONVENTIONS、SBOM、内部类型与签名、关键调用栈 | 通过 |
| `PROVIDER.html` | lower | TERMS、CONVENTIONS、DIAGRAMS、INTERFACES | 通过 |
| `BUILD.html` | — | 构建依赖、编译测试、示例、外部客户端、依赖接入 | 通过 |

说明：

- bnote 是无图形界面的库，README 不设 Prototype/mock；bnote 是项目群成员，不设 Project map。
- USER 不设 UI 章节，符合“仅图形程序”的可选条件。
- USER/CORE/PROVIDER 的 CONVENTIONS 章节均位于第一章之后；USER 与 PROVIDER 为空章节，GUIDELINES 允许。

## 2. 全局约束

| # | 约束 | 结果 | 核查说明 |
|--:|---|---|---|
| 1 | Single source of truth | 通过 | 类型/错误/协议定义分别落在 USER、PROVIDER、CORE；跨文档只保留一行声明与链接。`reference/` 为协议权威。 |
| 2 | No amendment history | 通过 | 未记录旧版本、迁移过程或对历史设计的比较。 |
| 3 | `reference/` | 通过 | `reference/index.html` 索引第 2–9 章，逐章链接官方原文；PROVIDER 只登记消费子集。 |
| 4 | HTML escaping | 通过 | 自动扫描未发现代码块中的裸 `&`、`<`、`>`；代码块内的 `<a>` 仅用于文件树链接。 |
| 5 | Notes | 通过 | 图、表、文件树、调用栈后的补充均使用 `<ul class="note"><li>…</li></ul>`。 |
| 6 | Section layout | 通过 | USER/PROVIDER/BUILD 仅开头有说明性段落；中段为代码、表格、图；结尾为 `ul.note`。CORE 无开头段落。 |
| 7 | Comment budget | 通过 | 已把超过 3 个信息点的代码注释移入 `ul.note`；剩余注释均为 1–3 个信息点。 |
| 8 | One block, one flow | 通过 | 各代码块按单一主题组织；CORE §5.8 的心跳流程已独立成块。 |
| 9 | CONVENTIONS chapter | 通过 | USER/CORE/PROVIDER 均保留该章节，位置正确；共享约定只在章节内声明。 |

## 3. 分文档检查

### README

- WHAT & WHY：包含 IDEAL、GOAL 与非目标。
- 用户交互：覆盖装配、事件消费、同步命令、客户端关闭、单消费者与 `NameTaken`。
- Prototype/mock：不适用（无图形界面、无原型）。
- Project map：不适用（成员项目，非仓库根）。
- Credits：存在且为最后一个内容章节；协议与 zbus 来源完整。

### USER

- TERMS：宿主、会话、通知、事件流、紧急度、动作、能力、关闭原因均有完整定义。
- CONVENTIONS：空章节，允许。
- DIAGRAMS：总体架构为 Component Diagram；装配/事件/回发为 Sequence Diagram；图源与导出图一致。
- INTERFACES：
  - 宿主 API 为 Rust 代码块；
  - `NoticeEvent`、`CloseReason`、`Error` 为消息/错误定义块并配含义表；
  - `Notice`、`SessionConfig` 等为数据模型代码块并配字段表；
  - 归一映射链接到 PROVIDER §4.3，未重复协议原文。
- UI：无，符合库的定位。

### CORE

- 文件树：覆盖 `Cargo.toml`、`examples/`、`reference/`、`tests/`、`src/`；源文件均链接到对应签名小节。
- CONVENTIONS：声明规范引用方式与公开面类型约束。
- SBOM：列出 `zbus`、`tokio` 与测试用 `futures-util`，版本/许可证保留在 `Cargo.toml`。
- 内部类型与签名：按文件树顺序排列；USER/PROVIDER 已定义的类型使用一行声明并带 `//见`；内部类型给出字段/方法签名。
- 调用栈：覆盖启动、通知到达、客户端关闭、宿主关闭、动作回发、激活令牌、停机、总线终止；行均为调用或判断，未保留 `mount` / `request_name` / `claim stop` 等阶段标签。
- 工程常量：`EVENT_QUEUE_CAPACITY` 保留名称/类型作为实现指导；取值不写入，参考值 64 仅以注释出现，实际值以代码为准。心跳 30s/5s 只作为调用栈内联字面量出现，符合例外规则。

### PROVIDER

- TERMS：D-Bus 会话总线、服务名、客户端、hints 均有定义。
- CONVENTIONS：空章节，允许。
- DIAGRAMS：协议边界为 Sequence Diagram，只出现客户端/总线/bnote 的方法与信号，不含 bnote 内部调用结构。
- INTERFACES：
  - 方法应答、信号回发、错误应答均为消费子集；
  - 参数归一与信号编码使用转换表，目标对象名与 USER §4.3 一致；
  - 协议全文链接到 `reference/` 与官方规范，未复制完整规范。

### BUILD

- 构建依赖：工具链、运行期依赖、测试依赖、运行环境、测试客户端齐备。
- 编译与测试：格式、构建、测试、Clippy、隔离总线集成测试命令完整。
- 运行示例：示例命令、stdin 命令、`NameTaken` 处理、隔离总线说明齐备。
- 外部客户端：`notify-send`、`dunstify`、`gdbus` 示例与注意事项完整。
- 作为依赖使用：依赖声明、运行时/执行器边界、文档链接齐备。

## 4. 图表与 reference

- 三张图均为 PlantUML 源文件 + SVG 导出，位于 `bnote/diagram/`。
- 重新导出后与仓库内 SVG 逐字节一致，源与图未分叉。
- 图表形式：总体架构为 Component Diagram；协议边界与宿主流程为 Sequence Diagram；无分支密集流程，无需 Activity Diagram。
- 标签为中文，API/协议名按需保留；嵌入均使用 `<div class="diagram"><img src="diagram/…">`，CSS 保持原始尺寸与横向滚动。
- `reference/` 为官方规范清理版分章镜像；入口、章节锚点与 PROVIDER/USER 引用一致。

## 5. 审查中完成的调整

| 位置 | 问题 | 处理 |
|---|---|---|
| `CORE §4.4` | 需要保留 `EVENT_QUEUE_CAPACITY` 作为实现指导 | 保留名称/类型；移除取值，参考值 64 仅作注释；`GUIDELINES.md` 同步澄清该规则 |
| `CORE §4.3` | `Bus` 字段注释含 5 个信息点，超出注释预算 | 移入同节 `ul.note` |
| `CORE §4.7` | `HintSet.icon` 注释含 4 个图标来源与回退信息 | 移入同节 `ul.note` |
| `CORE §4.5` | `State` 以 unit struct 占位，未体现内部结构 | 补 `next_id` / `active` 字段与 `allocate` / `track` / `contains` / `remove` 签名 |
| `CORE §4.8` | `NoticeEvent` 误写为 struct；`enum` 声明使用分号 | 改为带 `//见` 的 enum 声明 |
| `CORE §5.1` / `§5.2` / `§5.8` | 调用栈含 `mount`、`request_name`、`claim stop` 等阶段标签 | 替换为 `conn.object_server().at(...)`、`conn.request_name_with_flags(...)`、`State::allocate()` / `State::track(...)`、`Bus::claim_stop()` 等具体调用 |
| `CORE §5.8` | 心跳子流程与 Fatal 主流程混在同一代码块 | 拆为两个代码块，分别描述终止路径与心跳路径 |
| `CORE §3` / `BUILD §1` | SBOM/构建依赖未列测试用 `futures-util` | 补充为 dev-dependency |
| `README §3` | “参考资料”为模板外章节，且与 Credits 重复 | 删除该章节；reference 链接保留在 Credits，Credits 成为最后一个内容章节 |
| README/USER/CORE/PROVIDER/diagram | 文档与图混用 `DBus` 与 `D-Bus` | 正文与图标签统一为 `D-Bus`；保留 `org.freedesktop.DBus.*` 与 `DBusProxy` 等固定标识 |
| CORE/USER/BUILD 多处 | 说明句含“避免/因为/未来”等设计理由 | 改写为约束或事实陈述，去掉历史/未来理由 |

## 6. 观察项（不阻塞）

- CORE 的方法条目按“签名 + 目的说明”呈现，函数体/常量初值省略；GUIDELINES 已明确这些声明用于表达契约，不视为可独立编译的代码块。
- USER/PROVIDER 的 CONVENTIONS 为空章节，GUIDELINES 明确允许。
- `reference/` 镜像第 2–9 章；第 1 章为引言性质，索引中链接官方完整原文，消费子集以第 9 章为准。
- CORE 心跳周期/超时保留在调用栈内联注释中，属于“流程依赖的字面量”例外；实际值以代码为准。
- 工程常量的权威取值始终在代码；CORE 只保留名称/类型（省略初值），参考值仅可作为注释出现。

## 7. 验证

```text
本地链接/锚点检查            通过
HTML 裸 & / < / > 扫描        通过
PlantUML 重新导出与 SVG diff  通过（逐字节一致）
cargo fmt --all -- --check    通过
cargo clippy --all-targets -- -D warnings  通过
cargo test --all-targets      通过（未设置 BNOTE_TEST_DBUS 时集成测试按设计跳过）
```

## 8. 结论

当前 bnote 文档满足 `GUIDELINES.md` 的文档集合、章节结构、全局约束、图表与 reference 要求；无未关闭的阻塞项。
