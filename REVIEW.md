# docs/bnote 合并待决清单（工作稿）

> 定位：合并 `dev/bnote` 项目文档与 `docs/notification` 文档后的待拍板事项。已按「冲突优先 dev/bnote」暂定；处理完请删除本文件。
> 基线：`dev/bnote` @ `5f97094`；`docs/notification` @ `1b8d075`。
> 分支形态：`docs/bnote` 为纯文档分支；源码（`bnote/src`、`bnote/tests`、`bnote/examples`、`bnote/Cargo.*`）不纳入，代码留在 `dev/bnote` / 后续实现分支。

## 合并结果

| 来源 | 内容 |
|---|---|
| dev/bnote | 根 README / ARCHITECTURE / CODE / BUILD、`bnote/` 全套文档与图表、`style.css`、`GUIDELINES.md` |
| docs/notification | `TODO.md` 的「类 macOS 底部托盘」条目 |
| 冲突处置 | GUIDELINES 的 CORE 常量与调用栈规则取 dev/bnote；根文档中的通知中心契约取 dev/bnote |

说明：docs/notification 中较早的根文档版本（无 bnote 成员条目、无 NameTaken 重试）未逐字保留；除 GUIDELINES/TODO 外，其内容均已被 dev/bnote 版本覆盖或取代。

## 待拍板

### D1. GUIDELINES 的 CORE 规则采用哪版？

- **A（当前）**：dev/bnote 版——工程常量保留名称/用途，具体值只能作为注释中的推荐值，不得在声明中赋值；恢复 4 条调用栈细则（一行一调用、监听器绑定、共享路径独立根、结构跟随源码）。
- **B**：docs/notification「new edition」——常量“记录变量名与用途”，推荐值可出现在说明中；删除 4 条调用栈细则。
- **C**：混合——保留 4 条调用栈细则，常量措辞采用 docs/notification。

暂定 A。影响：`bnote/CORE.html` 按 A 编写；选 B 后 CORE 仍可读，但部分条目会显得过度约束。

### D2. 通知中心「NameTaken 不可用态 + 手动重试」是否保留？

- **A（当前）**：保留。ARCH / CODE / BUILD / README 保留 `notification_available`、`notification.retry`、刷新按钮与错误态；与 `bnote` 的 `Error::NameTaken` 及“宿主可再次调用 `Session::start` 重试”一致。
- **B**：采用 docs/notification 简化版——删除不可用态与重试 IPC，仅记录日志、按项降级。

暂定 A。若选 B，需同步删除 ARCH §3.10.6 / §4.4 / §4.5、CODE §3.5 / §3.12 / §3.16 / §3.17 / §4.5–4.6、BUILD 故障表与 README 注记中的对应内容。

### D3. `EVOLVE.md` 的去留

- **A（当前）**：原样保留。它是 docs/notification 分支的设计总结，§三仍有 9 项待拍板（弹窗堆叠顺序、通知中心水平位置、仅当前显示器的实现语义、组内排序/缓存上限、100% 屏高行为、`app_name` ↔ `class` 匹配、紧急卡片配色、色板措辞、关闭信号回发）。
- **B**：删除。该文件是工作记录，不是 GUIDELINES 五文档之一，且部分描述已过时。
- **C**：保留但改写为“历史记录 / 开放问题索引”，去掉已完成项。

暂定 A，避免合并时丢开放问题；若定稿前清理，建议 C。

### D4. `bnote/COMPLIANCE.md` 是否留在文档分支？

- **A（当前）**：保留。它是 2026-10-21 的文档合规复查记录，证明 bnote 五文档按 GUIDELINES 审过。
- **B**：删除。它是审校工作记录，且验证命令依赖未纳入本分支的源码。

暂定 A（随 dev/bnote 一并保留）。

### D5. `bnote/BUILD.html` 在纯文档分支是否保留？

- **A（当前）**：保留。BUILD 属于项目文档；命令指向 `dev/bnote` 中的 crate。
- **B**：删除。本分支无源码，无法直接执行构建/测试命令。

暂定 A。

## 已执行（供抽查）

- 合并提交以 docs/notification 为第一父、dev/bnote 为第二父；冲突按 dev/bnote 解决。
- 并入 docs/notification 的 TODO 底部托盘条目；GUIDELINES 冲突未采用其 new edition。
- 剥离 bnote 源码与 Cargo 清单，保留 bnote 文档、reference、diagram。
- 链接/锚点检查：相对前两支无新增断链；原有指向官方镜像内部锚点的断链保持不变。

## 回复方式

按 `D1-A、D2-A…` 回复即可；未回复项保持“当前”方案。
