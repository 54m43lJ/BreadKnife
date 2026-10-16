# 文档审校待决清单（工作稿）

> 定位：breadknife 四文档按 GUIDELINES 格式化后的待拍板事项；处理完请删除本文件。
> 范围：根文档（README / ARCHITECTURE / CODE）——breadpan/ 文档此前已按 GUIDELINES 审校，本轮未动。
> 工作稿不属于四文档体系；已执行部分见文末。

## 待拍板

1. **术语表（ARCHITECTURE §1）——已决：全部删除**。6 条术语无约束力，已移除；§1 仅保留指向 breadpan 定义的引用。连带修正：§5.1 与 README 中「严格预分配」的引用改为就地表述。
2. **通用约定（ARCHITECTURE §2）——已决并落地**：「主线程约束」作废，替换为「分层」（daemon＝Model，无 GTK；窗口进程＝View，GTK）与「操作执行原则」（有副作用操作必走 daemon；不影响 daemon 追踪状态的操作允许直通）两条。「缺失降级」保留。④ 已澄清：daemon 只回数据、不碰 GTK（菜单快照交窗口渲染）。全部细则（IPC＝Unix socket、托盘链路、生命周期、入口、通知、CSS 路径跟踪）已并入 ARCHITECTURE / CODE / README。
3. **章外内容去向——已决并落地**：模块规格（§5）按内容深度并入 README（用户可见行为：工作区三态定义、托盘提示与失联置灰、可辨底色）与 ARCHITECTURE 架构章（机制口径由 §3/§4 承接，不再另设章节）；样式系统（§6）并入 §3 作为两个小节（§3.9 样式基础变量 / §3.10 组件样式规范）；路线图（§7）直接删除。连带：总体架构改 PlantUML 并体现 breadpan 落位（成员库）；仓库结构章与依赖小图整体移除（与 CODE 文件树重合；此点按拍板为项目处置，未入 GUIDELINES）。GUIDELINES 增补「架构图用 PlantUML（组件图）」；骨架章号纠正——术语 1 / 通用约定 2 / 架构图 3 / 接口 4（CODE 清单同步为文件树 1 / 通用约定 2 / 签名 3 / 调用栈 4）。breadpan §3.1 总图一并转录 PlantUML。
4. **文档地图归属**：旧 ARCHITECTURE §10 与 README 重复，已从 ARCHITECTURE 删除；README 保留并补 breadpan 行。确认此归属。
5. **组装约定裁剪（README）**：原段落与 ARCHITECTURE §4.3 重复，已裁为注记指向 §4.3。确认。
6. **CODE 定稿时机**：顶部「状态：草稿（待评审定稿）」标识与 §1「文件树（规划）」字样是否随本次审校去除？
7. **CODE §3 符号引用格式**：未全面改用 breadpan 式统一引用（`pub <type> <name>; //见 ARCHITECTURE §x`），保留 Rust 形体（变体 / 字段完整）并加「协议语义见 §4」注记。是否收敛、收敛范围？
8. **调用栈伪码命名**：§4 已改写为调用式（`Tray::start()`、`spawn_window(mon)`、`build_modules(window)`、`control::listen()`、`write_pid()` 等）。请核对命名口径与将来实现的一致性。
9. **时序图 5 张（diagram/）**：命名 `breadknife-*`、参与者与消息请过目；布局如需调整，改 `.puml` 重导出即可。
10. **数据模型一处修正**：「工作区映射表」原表述（工作区 ID → 所属显示器）与 CODE `WorkspaceMap { by_monitor }` 方向相反，已统一为「显示器 → 工作区 ID 列表」。请确认以哪边为准。
11. **README 单段小节**：「运行管理」「个性化」为单段散文——是否列表化，或「运行管理」移入将来的 BUILD.html？
12. **悬空链接**：文档地图与 README 中的 BUILD.html 在当前分支尚不存在（按 GUIDELINES，BUILD 晚于实现）——保留指针即可？
13. **托盘调用线程化（已决议，随架构调整落位）**：全同步公开面保留；托盘调用整体位于 daemon（无 GTK——阻塞不冻结任何窗口），窗口经 IPC 委托、不受等待影响（见 ARCHITECTURE §4.4）。前提核查：公开数据面全 `Send`，唯一主线程例外为 `icon_paintable`（窗口侧渲染）。库侧条款已落（breadpan/ARCHITECTURE.html §2「同步封装」）；待办：应用侧按新架构实施（与 MIGRATION 联动）。
14. **库侧缺陷待修**：`Tray::start` 兜底超时路径可能悬挂（启动卡死）——源码核查发现，修法方向见 MIGRATION §6.8。

## 已按规则执行（供抽查）

- 应用三文档：补充文本统一 `ul.note`；跨章重复表述（缺失降级、组装顺序、样式状态色）收拢为单点 + 引用；
- 5 张 ASCII 时序图改为 PlantUML（`.puml` + `.svg` 同目录提交，`<div class="diagram">` 嵌入）；
- 删除历史性表述（「原独立库解散」）与重复章（ARCHITECTURE §10）；
- 校验：全仓链接 / 锚点检查通过（仅 BUILD.html 预期悬空）；`&amp;` 实体检查通过。
