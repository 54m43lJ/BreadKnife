# breadpan 集成动作目测（工作稿）

> 定位：把 breadpan 库落地进 BreadKnife 应用的粗粒度动作清单（工作稿）。
> 应用侧迁移细节以 [MIGRATION.md](MIGRATION.md) 为准；本文只列集成线条与先后关系，不重复其内容。

## 目标与范围

应用（`bread`）的托盘实现由 astal-tray 换成 breadpan，库随仓落位，最终合回 main，去掉系统级 libastal 构建依赖。
不做与迁移无关的重组（`src/modules/*` 规划属后续）；功能面档位见下方待拍板。

## 线条

1. **落位与合入路径**：库代码已随 `dev/breadpan-2` 就位（`breadpan/` 子 crate，与应用代码共存），但应用侧依赖尚未切换；
   集成 = 文档线并入 dev → 应用迁移 → 合回 main。CI（`.github/`）不在 dev 分支，合回时一并处理。
2. **应用侧迁移**：MIGRATION.md §4 的 P0–P6（依赖切换 → 托盘服务 → Item 生命周期 → 交互 → 菜单弹窗 → 联调/清理）。
3. **构建/CI**：preview workflow 删除 libastal/astal-tray 与 appmenu-glib-translator 构建步骤；接入库测试（`dbus-run-session -- cargo test`）。
4. **文案与文档**：main 的 `README.md`（旧 Astal 文案）随合入清理，README.html 为事实源；
   应用文档 ARCHITECTURE §4.3–§4.5 / CODE §4.4 / BUILD 依赖表已按 breadpan 契约写就，代码迁移后核对找平。
   demo-tray 项目已整体移除（含引用），验收操作台改用 breadpan 的 interactions 示例。
5. **验收**：MIGRATION.md §7 真机矩阵 + 对照旧 astal 版行为回归。

## 待拍板

- 迁移范围档位：最小等价（图标+左右键+菜单）vs 文档契约全量（含中键/四向滚轮/状态增强）（MIGRATION.md §8）。
- 菜单渲染路线：自绘 `gtk::Popover`（推荐）vs `GMenuModel` 复用（MIGRATION.md §6.1）。
- Fatal/Displaced 是否加自动重连（MIGRATION.md §6.5）。
- 集成分支形态：沿用 `dev/breadpan-2`，或从 main 另起集成分支。

## 参考

- [MIGRATION.md](MIGRATION.md)：应用侧 astal-tray → breadpan 迁移评估（P0–P6、API 对照、风险、验证计划）。
- [BOUNDARY.md](BOUNDARY.md)：breadpan 与 astal-tray 能力边界对照与应对方案。
- 库契约：breadpan/ARCHITECTURE.html、breadpan/CODE.html（BUILD.html 在 dev 分支随实现提供）。
