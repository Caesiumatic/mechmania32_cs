# MechMania 32 M1.1 + M2a / round 2 实施报告

本报告把实际执行结果写成“事实”，策略解释写成“推导”，尚未执行或需要外部裁决的事项标为 **OPEN**。路径均相对于仓库根目录；大型回放、构建树和日志保存在 Git 忽略的 `.local/`。

## 1. 状态与逐项门槛

**状态：`completed_with_open_items`。**

生产侧的精确射击查询、同 tick 火力去重、满血治疗者跟队和有效治疗优先分配均已落地；干净副本恢复、66 项测试、32 局默认配置正式诊断矩阵、2 局受控长局、packcheck 与综合审计均已执行。状态没有标为 `completed`，因为 v1 在本轮小样本矩阵中存在明确战术回退，并且官方 nightly、第二平台/在线空缓存、服务端验证和 R24 规则裁决仍是开放项。

| 门槛 | 状态 | 证据 |
|---|---|---|
| 来源与工具链 | PASS with warning | 上游提交/归档/checkout 树和本机构建记录均通过 audit；stable 1.98.1 可用，官方 nightly 与当前 lock 不兼容 |
| 冷项目恢复 | PASS（限定范围） | `clean-761c8f8` 按 bootstrap → test → timed match → packcheck → bootstrap 执行，5 步 exit 0 |
| 几何修复 | PASS | D01-D05、256 seed oracle；纯 scan 误差 0，eval_tick 最大误差 0.000027771415 |
| 治疗修复 | PASS | D06-D10 与 R05 的 3.0 / 45° 内外边界 |
| 测试回归 | PASS | bot 17、rules 26、opponents 5、tools 18，共 66/66 |
| 对战矩阵 | PASS（证据完整，不代表强度通过） | 默认配置 8 + 24 = 32/32，有换边、重复、逐局 callback 与结果 |
| 长局/终局 | PASS（formal + scenario 分列） | 正式矩阵 16 局 ≥6000、6 局到 9000；受控 soak 2 局均到 9000 |
| 预算 | PASS | 所有批次 skipped tick=0，最低储备 500000，峰值最高 73 |
| 打包闭包 | PASS | 干净官方骨架只恢复 strategy，release build exit 0，源/复制树哈希一致 |
| 仓库审计 | PASS with warnings | failures=[]；秘密、大文件、输入过期、M1 历史保护、来源树与构建记录均通过 |
| Git 交付 | PASS | 用户要求保持 `main`；代码提交与证据提交分离，最终 delivery SHA 由 Git 历史和交付回复给出 |

本地范围没有阻塞项。官方服务端提交没有执行，因为任务明确要求另行取得 Shi 授权。

## 2. 被测代码、仓库、环境与上游

### 2.1 Git 语义

| 项目 | 实际值 |
|---|---|
| repository | `Caesiumatic/mechmania32_cs` |
| branch | `main`；按用户要求未创建或切换分支 |
| 本轮 base | `e789961d7dedb00f70b1947dfd1435bc68a65cfe` |
| 历史 v0 代码 | `d13c2360e1ce5708078b45254fc862de8ce8341f` |
| evaluated code commit | `761c8f80b01fbbf5d50cd2e4b4b719d269e7ea08` |
| delivery commit | 本报告所在的后续证据提交；提交内容不能可靠自引用自身 SHA，以最终 Git 回执为准 |

正式测试、32+2 局、packcheck 和 cleancheck 都绑定 evaluated code commit。后续只新增报告与精简证据，符合“两提交语义”。

### 2.2 输入文档

- prompt2 引用的 `mechmania32_review1.md` 本轮没有提供，仓库中也不存在可导入原文；状态记为 `not_provided`，没有生成假评审。prompt2 自身列出的 A/B/C 必修发现作为实现集合。

### 2.3 冻结上游与产物

| 来源 | 固定提交 |
|---|---|
| engine runtime | `4cbd3467e6617e330d42cb78ae3327ab2c88926a` |
| bot client | `879578d448251d0acc7f10eef3d7fe5450758096` |
| rust starterpack | `f9960353f414938002ea2bb18161b6851a71c40c` |
| Wiki | `6af083bb4c28a4e18d2f7fcdda5a380aebc0545f` |
| CLI | `4c5bb2f33d7fdee10e9d8f311f382ff198b81214` |

Audit 对四个固定 tarball、关键 Cargo/Wiki 文件和展开后的完整 checkout 树都重算并通过。最终本机 engine 二进制 SHA-256 为 `5b1b8b05ef6df9929715d724fd80d17fce416f04331ebeac2dd779e295262e76`；v1 release / packcheck 二进制 SHA-256 为 `33e2fddef4923523a79d254a32bebd0401e27094429b21fc7618880f8ae76d6c`。

### 2.4 实际工具链

- OS / architecture：Darwin `25.3.0` / arm64；target `aarch64-apple-darwin`。
- 实际使用：Rust `1.98.1`、Cargo `1.98.1` stable、release profile。
- compiler fingerprint：`71aa34f5268400fc443d3d54d309a470e6d8bab43bb6fbb46b300f4239c5633a`。
- toolchain fingerprint：`c0199d6128ca2eed3c839a40d00e93a4fdb9615bd24ff59881f12c4a66f64444`。
- `configs/toolchain.lock.json` 记录精确可执行路径、共享安装/项目目录来源、版本输出与环境覆盖。

**OPEN**：上游声明的 `nightly-2024-07-25` 所带 Cargo 无法解析当前锁文件中的 edition-2024 依赖（包括 `clap_lex 1.1.1`）。本轮没有私改官方 lock 来制造通过；doctor 分别报告“本地 stable 可用”和“官方声明工具链不兼容”。

## 3. 评审发现、修复位置与前后证据

### A1 工具链选择

`tools/mm32.py` 现在依次处理显式配置、调用者已安装工具和受控 fallback；保留实际可执行路径，并记录 `rustc -Vv`、Cargo、target、OS、架构、profile、Cargo/Rustup home 与覆盖变量。`doctor` 不再把本地可用性与旧 nightly 兼容性混成一个布尔值。

### A2 来源锁与本机产物记录

`configs/upstream.lock.json` 锁源码提交、归档、Cargo 文件与固定配置；bootstrap 生成的 engine/CLI build record 另存平台、完整源码树指纹、工具链、命令和实际二进制哈希。跨平台不再要求二进制等于一份 Darwin arm64 历史哈希，同一批次仍固定实际二进制。

### A3 冷副本验收

见第 5 节。起始副本只含 Git 跟踪内容，明确验证 `.local/`、`bot/Cargo.toml`、`bot/Cargo.lock`、`bot/src/core.rs` 与 engine 二进制缺失；五步全部执行成功，第二次 bootstrap 幂等。

### A4 runner 与历史证据

`tools/mm32.py` 的 test/smoke/packcheck/audit 支持 `round`、`suite`、`run-id`；对手可来自单文件或固定提交的完整 strategy 树。历史 `m01_v0` 从 `d13c2360…` 提取，Git tree OID `233a69435e0d2dd0863959ea4114a341140ffbdd`，canonical tree SHA-256 `9bbc438e5e66905e24ee5b3e1449c356cc4848351b6bfb2fbdac0e42e04ca4b8`，构建二进制 SHA-256 `05f173c382c123f3586a24dd13a3114f683c4c159a36ba9dada3a2e93f1c0130`。

输入指纹覆盖实际生产树、规则、对手、fixture、suite、工具、工具链和来源；每个子命令的最新 attempt 必须成功。`4e66145` 的早期成功批次保留但标为 superseded，最终选择 `761c8f8` 批次；M1 证据没有被改写。

### B1-B6 精确射击

修复前证据绑定 v0 的真实 `tactics.rs` SHA 与完整策略树，保存在 `artifacts/summaries/m02/pre_fix_ballistics.json`：

- D01：旧实现把 0.55 当整条射线的溅射管，错误开火。
- D02a / D02b：旧中心投影/距离预过滤错误拒绝外壳首碰和端点溅射；D02c 正确拒绝 10.56。
- D03：旧 Boolean 无法表达受保护近敌先截断炮线。
- D04：旧目标中心 LOS 不能表达限速转向后的炮线先撞墙。
- D05：旧实现只登记意图目标，不能对群体爆区做边际覆盖去重。

生产新增 `ballistics.rs`，复刻固定引擎的墙遍历、边界、敌方圆、载具、矿点、严格 tie、射程端点和爆区 mask；友军可穿透、受保护敌人仍参与首碰。`tactics.rs` 只把敌方下一步位置当行为模型，查询本身保持纯几何。中央调度器按新增可承伤集合、击杀、治疗者、直接命中权重做小规模贪心，同 tick 扣除已覆盖集合。

修复后 D01-D05 共 7 项通过；256 个固定生成 seed 与官方 oracle 对照，纯 scan 爆点最大误差 `0.0`，`eval_tick` 最大误差 `0.000027771415 < 0.001`，首碰/物理 mask/可承伤 mask 差异均为 0。

动作级限制如实保留：复杂敌方动态碰撞没有完整投影；己方临近载具使用 hold / ±1 tick 位置三态，三态不一致时保守不射并计数。另有一个遥测缺口：自身 firing-origin 因碰撞不确定而提前返回时没有递增 `geometry_uncertain_candidates`，因此不确定性计数可能偏低。

### C1-C4 治疗协同

`coordination.rs` 将“移动支援锚点”和“本 tick 治疗目标”分离。全队满血时，治疗者仍跟随战斗群/载具任务；锚点和治疗目标都有切换惩罚。调度先用全队统一预测位置保留当前可兑现通道；范围边缘治疗者原地保持，不会为长期锚点丢掉当 tick 治疗；剩余治疗者才赶路。第二阶段再按缺血量和三通道上限分配实际目标。

D06-D10 全部通过，覆盖满血跟队、近处有效治疗优先、移动/转向/三通道/无效目标、槽位复用与跳 tick 缓存、载具/矿点邻近合法支援点。规则侧另把 R05 拆成实际断言：中心距恰好 3.0 和半角恰好 45° 均包含，刚越界均拒绝。

C4 没有扩展成大规模阵型搜索；只加入为碰撞合法性、队伍支援和载具分散所需的静态参数。self-destruct 仍关闭；学习参数数量仍为 0。权威策略参数集中在 `policy.rs`，ID 为 `m02-v1-static-2026-09-19`。

## 4. 测试计数、规则状态与覆盖积压

最终 `tools/mm32.py test --round m02 --suite ...` 对每个正式批次记录相同的已通过测试快照：

| 类别 | passed | failed | skipped |
|---|---:|---:|---:|
| bot | 17 | 0 | 0 |
| rules | 26 | 0 | 0 |
| opponents | 5 | 0 | 0 |
| tools | 18 | 0 | 0 |
| 合计 | 66 | 0 | 0 |

Rules 的 26 项由 19 个引擎合同（M1 的 17 加两个 R05 精确边界）和 7 个 M2 `policy_geometry` 用例构成。另执行 rustfmt check、release build 与 `git diff --check`，均通过；本机没有安装 clippy，因此不把 clippy 写成已执行门槛。

覆盖积压：动态实体的敌方碰撞投影、跨 tick 火力预约、更强随机/扰动对手、第二 OS/架构，以及 R24 终局采矿裁决。详细 CODE/DOC/TESTED/OPEN 区分见 `docs/rules/mechmania32_rules2.md`。

## 5. 冷恢复与 packcheck 的精确记录

### 5.1 tracked-only 冷项目

`artifacts/summaries/m02/cleancheck/clean-761c8f8/manifest.json` 绑定 `761c8f80…`。输入归档 SHA-256 为 `132729be80b62757d1380e5ea87377862c5803141e47c9860d00281de917f706`。起始缺失检查在注入固定来源包之前完成。

| 顺序 | 命令标签 | 时长（秒） | exit |
|---:|---|---:|---:|
| 1 | `bootstrap_first` | 31.355366 | 0 |
| 2 | `test` | 39.310687 | 0 |
| 3 | `timed_match` | 23.774126 | 0 |
| 4 | `packcheck` | 11.855370 | 0 |
| 5 | `bootstrap_second` | 0.238359 | 0 |

策略树恢复前、首次 bootstrap 后、第二次 bootstrap 后 SHA-256 均为 `234dc827ea6373cb1ee11f48cb0761f399102c7eb3c5bb1bcd4e98dd7c81ddc6`；第二次幂等为 true。

复现范围限定：四个固定源码 tarball 在初始缺失核验后从可信父项目种入，Cargo network 被禁用；Cargo/Rustup home 位于干净项目 `.local/`，但实际 rustup 可执行与已有依赖/工具链缓存来自已记录的共享安装。因此这证明“仅跟踪项目 + 固定来源包 + 已记录工具链/包缓存”的离线恢复，不等于第二平台或完整在线空缓存下载。

### 5.2 最终 packcheck

最终 `challenge-761c8f8` packcheck 在临时官方 starterpack 中只恢复 `bot/src/strategy/`；源树和复制树 SHA-256 都是 `234dc827…`。stable release build 用时 11.648760 秒、exit 0、无新增 warning；产物 SHA-256 为 `33e2fd…`。结论只到“本地打包闭包通过”；server submission 状态为 `not_run_requires_shi_authorization`。

## 6. 默认配置对战结果

所有正式比赛都使用同一冻结 runtime、默认 fixture、release profile 和 client 版本；完整 callback 覆盖、退出码、正式时间限制和官方错误日志检查均通过。固定引擎没有 seed CLI 参数，因此策略/矩阵/二进制/重复次数固定，但进程 RNG seed 不可显式重放。

### 6.1 回归批次：8/8

| candidate | opponent | side | 结果 × 局 | 结束原因 | final tick |
|---|---|---|---|---|---:|
| v1 | `starter_symmetric` | A | 负 ×2 | payload | 3829 |
| v1 | `starter_symmetric` | B | 负 ×2 | payload | 3822 |
| v1 | `rush_simple` | A | 胜 ×2 | payload | 3859 |
| v1 | `rush_simple` | B | 胜 ×2 | payload | 3866 |

汇总：v1 对 starter 为 `0W-0D-4L / 0 points`；对 rush 为 `4W-0D-0L / 12 points`。

### 6.2 挑战批次：24/24

| candidate | opponent | side | 结果 × 局 | 结束原因 | final tick |
|---|---|---|---|---|---:|
| m01_v0 | m01_v0 | A | 负 ×2 | health_pool | 9000 |
| m01_v0 | m01_v0 | B | 胜 ×2 | health_pool | 9000 |
| m01_v0 | support_screen | A | 胜 ×2 | payload | 3293 |
| m01_v0 | support_screen | B | 胜 ×2 | payload | 3293 |
| m01_v0 | strafe_pressure | A | 负 ×2 | elimination | 6504 |
| m01_v0 | strafe_pressure | B | 胜 ×2 | payload_progress | 9000 |
| v1 | m01_v0 | A | 负 ×2 | payload | 6083 |
| v1 | m01_v0 | B | 负 ×2 | elimination | 6192 |
| v1 | support_screen | A | 胜 ×2 | payload | 3235 |
| v1 | support_screen | B | 胜 ×2 | payload | 3235 |
| v1 | strafe_pressure | A | 负 ×2 | elimination | 6584 |
| v1 | strafe_pressure | B | 负 ×2 | elimination | 6584 |

积分汇总：

| candidate vs opponent | W-D-L | points |
|---|---:|---:|
| m01_v0 vs m01_v0 | 2-0-2 | 6 |
| m01_v0 vs support_screen | 4-0-0 | 12 |
| m01_v0 vs strafe_pressure | 2-0-2 | 6 |
| v1 vs m01_v0 | 0-0-4 | 0 |
| v1 vs support_screen | 4-0-0 | 12 |
| v1 vs strafe_pressure | 0-0-4 | 0 |

这是诊断矩阵，不支持总体强度或显著性结论。它足以说明：v1 的规则修复没有自动转化成竞技收益；尤其 v1 对历史 v0、strafe 和 starter 的 12 局全负是需要下一轮解释和调参的真实回退。

## 7. 长局、预算、行为指标与观测缺口

### 7.1 长局与高人口

挑战批次中 16/24 局达到或超过 tick 6000，6 局到 tick 9000。另建受控 soak：默认配置只改 `payload.speed: 0.02 → 0.0` 和 `bot.blaster_damage: 3.0 → 0.0`，分类为 `soak_scenario`，不与默认正式比赛合并。

soak 两局都是 v1 对同批冻结 v1 换边，均到 9000，一胜一负（tokens 决胜）；最大友方人口 32，高人口（≥24）累计 15198 tick，控制圈争夺累计 17028 tick。默认正式比赛还覆盖 6000 停产边界、health/tokens/progress 决胜和 elimination；槽位复用、目标消失和跳 tick 恢复由控制器单元测试补足。

### 7.2 CPU 预算

| 批次 | calls | weighted mean | p95 max | p99 max | peak | min reserve | skipped |
|---|---:|---:|---:|---:|---:|---:|---:|
| regression | 30,752 | 7.8843 | 32 | 41 | 73 | 500,000 | 0 |
| challenge | 144,006 | 7.0314 | 32 | 37 | 62 | 500,000 | 0 |
| soak | 18,000 | 26.5301 | 32 | 35 | 41 | 500,000 | 0 |

即使高人口 soak，均值和尾部仍远低于内部参考目标（mean 300 / p95 600）。低储备分支会减少候选集合，但本批次没有进入低储备；不能据此断言该分支在正式长局中已被触发验证。

### 7.3 低成本行为遥测

- regression：开火请求/观测 `414/414`；爆点行为误差 mean `0.00000962`、max `0.00026104`；治疗请求/观测/删失 `16782/16762/20`；支援半径首次到达 tick 14；最大人口 28。
- challenge：开火请求/观测 `738/738`；爆点行为误差 mean `0.00856013`、max `0.71070093`；预测/实际新增目标 `1074/906`；治疗请求/观测/删失 `30034/29982/52`；有效容量占用 `0.3266269`；最大人口 32。
- soak：开火请求/可观察/待观测 `3042/3038/4`；最大人口 32。因 fixture 把炮击伤害设为 0，字段 `actual_new_damaged_targets=5626` 实际由“新 shot contact 或保护状态转换”重建，是接触/保护代理，**不是健康值下降数量**。

终局最后动作、单位同 tick 死亡等没有下一帧时记为 censored/pending，不算普通失败。比赛级爆点误差包含敌方匀速模型和未完整建模的动态碰撞；纯几何 oracle 的误差另列，不能混用。

## 8. 失败局归因、回退与保留决定

### 8.1 已观测回退

- v1 对历史 m01_v0：0-4；两侧分别被 payload 和 elimination 终结。
- v1 对 strafe_pressure：0-4，全部 elimination。
- v1 对 starter_symmetric：0-4，全部 payload。
- v1 对 support_screen 与 v0 一样是 4-0；对 rush 保持 4-0。

当前证据说明问题是战术/阵型层而非几何合同失败：纯 ballistics oracle 全通过，正式比赛也无 callback、预算、进程或非法动作失败。可能因素包括保守候选在动态接触区拒绝过多、`ENGAGE_RANGE_FRACTION`/阵型槽位与旧经济节奏耦合，以及治疗跟队改变了载具附近的单位分布；这几项是基于现象的假设，尚未由消融验证。

### 8.2 保留而不回滚

保留以下修复：

1. 官方一致的射线/端点/爆区查询与同 tick 边际覆盖，因为它们修复了可复现错误并通过独立 oracle。
2. 当前有效治疗优先、满血跟队与缓存失效，因为 D06-D10 / R05 精确合同通过。
3. 来源/工具链分离、冷恢复、冻结 v0/对手、证据指纹、行为遥测与综合 audit，因为它们提高后续实验可信度。

不把 v1 标为竞技升级，也不隐藏 0-4 结果。下一轮应冻结上述正确性层，对 engagement/formation/tactical weights 做受控消融和重新评估；若要部署竞赛版本，应由 Shi 根据强度证据另行决定，而不是本轮自动提交。

### 8.3 开放项与非阻塞限制

- 官方 nightly 与当前依赖锁不兼容。
- 没有第二 OS/架构，也没有完整在线空缓存恢复；当前沙箱网络探针不可用。
- 官方 server-side validation 未获授权，未执行。
- review1 原文未提供。
- 动作级敌方动态碰撞预测不完整；不确定性遥测有一个低报路径。
- R24 终局采矿仍是组织方 CODE / Wiki DOC 冲突。

这些项目使状态为 `completed_with_open_items`，但本地交付 `blockers=[]`。

## 9. 下一轮最值得处理的三个问题

1. **在不动正确性层的前提下恢复竞技表现。** 对 `ENGAGE_RANGE_FRACTION`、阵型半径/槽位、保守碰撞拒绝和治疗支援权重做小规模消融；用同一 32 局矩阵加更多重复验证，优先解释 v1 对 v0/strafe/starter 的失败模式。
2. **补齐动作级动态几何和可观测性。** 对敌方墙/载具/矿点碰撞投影建立官方单 tick oracle；让 firing-origin 的全部保守拒绝路径进入 uncertainty 计数；分开报告行为模型、碰撞模型和纯射线误差。
3. **关闭环境与外部规则缺口。** 协调官方 nightly/lock，随后在第二平台和真正空的在线 cache 重跑；取得 Shi 对服务端 pack validation 的授权，并向组织方确认 R24 终局采矿语义。

## 10. 证据入口

- 聚合指标：`artifacts/summaries/m02/metrics.json`
- 聚合清单：`artifacts/summaries/m02/manifest.json`
- 最终审计：`artifacts/summaries/m02/audit.json`
- 规则表：`docs/rules/mechmania32_rules2.md`
- 当前反馈：`artifacts/feedback/latest.json`
- 规范运行：`regression-761c8f8`、`challenge-761c8f8`、`soak-761c8f8`、`clean-761c8f8`

同目录中 `4e66145` 运行是修正最终代码提交指纹前保留的成功历史尝试；最终结论只选择当前 `761c8f8` 证据。
