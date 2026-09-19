# MechMania 32 M1 规则合同审计

## 1. 范围与证据基线

本文件记录 M1 已执行的规则合同，而不是对 Wiki 的重新解释。结论分为：

- **CODE**：固定引擎源码在实测场景中的行为。
- **DOC**：本轮提示词与 Wiki 快照所描述的预期。
- **OPEN**：代码与文档不一致，或当前测试不足以裁决的部分。

固定来源：

- 引擎提交：`4cbd3467e6617e330d42cb78ae3327ab2c88926a`。
- 独立规则工程：`lab/rules/Cargo.toml`；`mm-engine` 使用完整 `rev`，关闭默认 feature，仅启用 `engine`。
- 默认配置 fixture：`lab/fixtures/default_config.json`，来自固定官方引擎二进制输出的首行 `GameConfig`；SHA-256 为 `ab2839fc9e2fa4214e694225120f3c7112ee587831f1bcb58cc6602bdf33dfc0`。
- 几何边界测试从该真实配置复制后显式修改地图或矿点位置；这些局面是合成 fixture，不代表正式地图布局。

本轮实际命令：

```bash
PATH=/opt/homebrew/opt/rustup/bin:/usr/bin:/bin:/usr/sbin:/sbin \
RUSTUP_HOME="$PWD/.local/rustup" \
CARGO_HOME="$PWD/.local/cargo" \
CARGO_TARGET_DIR="$PWD/.local/target/rules" \
cargo +stable test --locked --manifest-path lab/rules/Cargo.toml
```

结果：规则合同 `17 passed; 0 failed; 0 ignored`，命令退出码为 0。独立的 bot 测试命令为：

```bash
cargo +stable test --manifest-path bot/Cargo.toml
```

结果：bot 单元测试 `5 passed; 0 failed; 0 ignored`，其中 R26 和移动方向 sanitize 回归测试通过。以下每条规则引用的 `C1` 指规则合同命令，`C2` 指 bot 测试命令。

## 2. 已执行规则合同

### R01 初始状态与 tick 0 免费生产

- **CODE 结论**：`GameState::new` 创建空舰队、两个生产计时器均为 0；首次 `eval_tick` 在 tick 0 各免费生产一台，类别采用该 tick 的 `fabricator_next`，Battle、Healer、Extractor 均已实测。
- **DOC 描述**：初始舰队为空；tick 0 自然生产到期；默认类别为 Battle，但动作可选择三种类别。
- **测试函数/命令**：`r01_initial_state_and_tick_zero_free_production`；C1。
- **实际结果**：**PASS**。三种类别分别建局验证，生产后 tick 为 1、舰队长度为 1、初始 token 未扣除、下次自然生产为配置 interval。
- **4cbd 来源函数**：`game/state.rs::GameState::new`；`game/action.rs::{eval_tick, step_fabricators, spawn_bot}`。
- **残余问题**：未在本条测试正式地图出生点的后续导航质量；该问题属于策略和移动测试。

### R02 同 tick 加急与自然生产

- **CODE 结论**：同一舰队先处理加急，再处理自然生产；两者同 tick 相撞时只新增一台、只扣一次 `rush_cost`，自然生产被推迟到下一 tick，不被吞掉。
- **DOC 描述**：加急和自然生产共享本 tick 的类别，每队每 tick 最多生产一台；相撞时自然生产后移。
- **测试函数/命令**：`r02_rush_and_natural_build_share_one_tick_and_defer_natural`；C1。
- **实际结果**：**PASS**。碰撞 tick 只生成 Healer，token 正确归零，计时器推迟 1；下一 tick 自然生产按新动作生成 Extractor。
- **4cbd 来源函数**：`game/action.rs::{step_fabricators, spawn_bot, eval_tick}`。
- **残余问题**：未覆盖加急资金刚低于价格、满人口加急、终局加急的所有排列；终局拒绝生产由 R18 覆盖。

### R03 满人口生产持有

- **CODE 结论**：32 台满人口时，到期自然生产不会消耗，`next_bot_creation` 保持在过去；释放一个槽位后，下一 tick 立即补回一台并重设计时。
- **DOC 描述**：满人口保留一次待兑现自然生产；释放后生产一台并重设计时。
- **测试函数/命令**：`r03_full_population_holds_due_build_until_a_slot_opens`；C1。
- **实际结果**：**PASS**。满员 tick 后仍为 32 台且计时器未前移；移除 slot 0 后下一 tick 恢复满员，计时器设为当前生产 tick 加 interval。
- **4cbd 来源函数**：`game/action.rs::step_fabricators`；`game/state.rs::BotArray::{is_full, remove, add}`。
- **残余问题**：尚未单独验证被击毁、主动自毁、手工移除三种释放路径是否在同一时点兑现；当前测试直接释放槽位。

### R04 新单位动作隔离

- **CODE 结论**：`eval_tick` 在生产前冻结旧单位的 `(team, BotId, action)` 列表；本 tick 新单位不会消费该空槽位中遗留的移动、开火或自毁命令。
- **DOC 描述**：新造单位的主动动作从下一 tick 起执行；旧槽位动作必须隔离。
- **测试函数/命令**：`r04_new_bot_ignores_stale_action_in_its_slot`；C1。
- **实际结果**：**PASS**。slot 0 预置移动、开火、自毁后，tick 0 新单位仍存活、速度为零、无射击记录且位置保持出生点。
- **4cbd 来源函数**：`game/action.rs::eval_tick` 中生产前的 `bot_actions` 快照；`step_fabricators`。
- **残余问题**：死亡后的 BotId 跨 tick 复用尚无独立合同，列入测试积压。

### R05 治疗自身与范围/朝向

- **CODE 结论**：Healer 不能治疗自身；另一台友军只有在中心距离不超过 `base_heal_range`、方向偏差不超过半个 `base_heal_arc_deg` 且静态视线无墙时才收到治疗。
- **DOC 描述**：治疗目标必须是另一台友军；中心距上限 3，完整治疗弧 90°，即朝向偏差至多 45°。
- **测试函数/命令**：`r05_healing_rejects_self_and_respects_range_and_facing`；C1。
- **实际结果**：**PASS**。自疗量为零；正前方 2 单位友军增加 0.05；60° 偏离目标不增加生命。
- **4cbd 来源函数**：`game/action.rs::step_healers`；`game/geom.rs::scan`；`game/util.rs::diff_degrees`。
- **残余问题**：本条未穷举恰好 3.0 和恰好 45° 的浮点等号边界，也未单列墙体遮挡。

### R06 治疗上限与执行顺序

- **CODE 结论**：单目标每 tick 的累计治疗封顶为 `heal_stack_cap × heal_per_tick`；治疗在炮击伤害之前应用，可使原本会被本 tick 炮击击杀的单位存活。
- **DOC 描述**：每台 Healer 每 tick 回复 0.05，同一目标最多三份；先治疗后受伤。
- **测试函数/命令**：`r06_healing_caps_at_three_and_resolves_before_damage`；C1。
- **实际结果**：**PASS**。三台和四台 Healer 的总回复同为 0.15；目标先从 2.975 回复至 3.025，再承受 3.0 伤害，以约 0.025 生命存活。
- **4cbd 来源函数**：`game/action.rs::{step_healers, step_blasters, eval_tick}`；`eval_tick` 的调用顺序明确先治疗后伤害。
- **残余问题**：尚未测试治疗与多个同时爆点、满血截断及多目标分配的全部组合。

### R07 伤害保护与开火冷却

- **CODE 结论**：同一 tick 的多发爆炸对一个目标只造成一次伤害；命中后保护至绝对 tick 15，tick 14 仍免伤、tick 15 恢复承伤；开火冷却至绝对 tick 60，tick 59 不能开火、tick 60 可以。
- **DOC 描述**：命中保护 15 tick，开火冷却 60 tick；同 tick 多发只扣一次。
- **测试函数/命令**：`r07_damage_protection_and_weapon_cooldown_boundaries`；C1。
- **实际结果**：**PASS**。同 tick 两枪只扣 3.0；保护和冷却的前一 tick/到期 tick 均按上述边界执行。
- **4cbd 来源函数**：`game/action.rs::step_blasters`；`game/state.rs::BotState::{next_fire_tick, shot}`。
- **残余问题**：配置为零冷却或零保护的异常配置未单测；代码用 `.max(1)` 保证同 tick 保护，但未在本轮单独锁定。

### R08 同 tick 互相击杀

- **CODE 结论**：所有旧 Battle 的爆点先按伤害前状态确定，再统一应用伤害和移除，因此同 tick 被击杀的单位仍能完成自己的炮击。
- **DOC 描述**：同 tick 互相击杀时双方都完成炮击，不因动作遍历顺序只留下单方。
- **测试函数/命令**：`r08_same_tick_mutual_kill_allows_both_shots`；C1。
- **实际结果**：**PASS**。双方均以一发伤害量的生命互射，完整 tick 后两个舰队均为空。
- **4cbd 来源函数**：`game/action.rs::{step_blasters, eval_tick}`。
- **残余问题**：未将互杀与治疗、采矿、载具控制放入同一综合 tick；各阶段顺序分别由其他合同覆盖。

### R09 射程端点溅射

- **CODE 结论**：射线未首碰实体时在 `blaster_range` 端点爆炸；有效中心距离上界为 `10.0 + 0.25 + 0.3 = 10.55`。中心距 10.50 的目标受伤，10.56 的对照不受伤。
- **DOC 描述**：达到射程端点也会爆炸；当前射程 10、单位半径 0.25、爆炸半径 0.3，开阔几何的中心距离上界为 10.55。
- **测试函数/命令**：`r09_max_range_endpoint_creates_splash_with_10_55_center_limit`；C1。
- **实际结果**：**PASS**。10.50 对照扣除 3.0，10.56 对照保持满血；测试同时断言计算上界为 10.55。
- **4cbd 来源函数**：`game/action.rs::step_blasters`；`game/geom.rs::scan`。
- **残余问题**：斜向射线及 `f32` 恰好 10.55 的舍入行为未单独锁定，当前测试使用 0.01 余量。

### R10 首碰物体与群体爆炸

- **CODE 结论**：公开 `scan` 和炮击执行均确认载具、矿点、敌军可成为首碰爆点；同一爆点范围内的多个敌军均受伤，友军既不阻挡己方射线，也不受友方溅射。
- **DOC 描述**：炮击可穿友军；首碰敌军、墙、边界、载具或矿点爆炸；爆炸只影响爆点周围的敌军集合。
- **测试函数/命令**：`r10_first_payload_deposit_or_enemy_sets_blast_and_only_enemies_take_splash`；C1。
- **实际结果**：**PASS**。分别验证 `ScanTarget::Payload`、`ScanTarget::Deposit`、`ScanTarget::Bot` 及记录的爆点；两个重叠敌军各扣 3.0，重叠友军保持满血。
- **4cbd 来源函数**：`game/geom.rs::{scan, ScanMask, ScanTarget}`；`game/action.rs::step_blasters`。
- **残余问题**：墙和地图边界虽在正式扫描 mask 中，但本轮 R10 没有再写独立外部合同；可在后续几何边界套件补齐。

### R13 载具推进与争夺

- **CODE 结论**：只有一方在控制圈内时，推进量只取人数差的符号，因此一台和多台同速；只要双方各有至少一台在圈内，载具立即停驶。
- **DOC 描述**：单边占领以固定速度推进；双方都有单位则停驶；控制判定使用单位中心距离。
- **测试函数/命令**：`r13_payload_one_or_many_push_at_fixed_speed_and_opposition_contests`；C1。
- **实际结果**：**PASS**。1v0 与 3v0 的 capture 增量相同且为正；1v1 与 3v1 均保持零进度。
- **4cbd 来源函数**：`game/action.rs::step_payload`；`game/config.rs::{payload_position, payload_path_len}`。
- **残余问题**：控制圈精确边界、移动后进入/离开控制圈及配置路径常量耦合尚未独立测试。

### R16 共享矿槽与占用粘性

- **CODE 结论**：单个矿点的 8 个槽由两队共享；仍满足采矿条件的旧占用者先保留，随机顺序只决定剩余槽的新进入者；每个有效槽每 tick 产生 0.05 token。
- **DOC 描述**：共享槽位总数为 8；已有合格占用者优先；新进入者只竞争剩余槽；满采收入为 0.4/tick。
- **测试函数/命令**：`r16_shared_mining_cap_preserves_incumbents_and_admits_only_free_slots`；C1。
- **实际结果**：**PASS**。8 名旧占用者完整保留并获得 0.4；新增两队竞争者均被拒；释放一个旧占用后恰有一个随机新进入者取得空位，总占用仍为 8、总收入仍为 0.4。
- **4cbd 来源函数**：`game/action.rs::step_extractors`；`game/geom.rs::scan`；`game/state.rs::Deposit`。
- **残余问题**：不声明具体随机获胜者；墙遮挡、转向释放、跨矿点切换和死亡释放未在本条外部合同中逐一拆分。

### R17 炮击阵亡加自毁

- **CODE 结论**：同一单位本 tick 被炮击移除后，后续自毁阶段先检查槽位仍否存在，不会再次调用 `remove`，因此完整 tick 不 panic 且只移除一次。
- **DOC 描述**：`4cbd3467…` 修复同 tick 被炮击击杀又请求自毁的重复移除问题。
- **测试函数/命令**：`r17_blast_death_plus_self_destruct_removes_slot_once`；C1。
- **实际结果**：**PASS**。受击方以恰好一发伤害量的生命请求自毁，tick 正常结束；该槽位为空，射手仍存在，无双重移除 panic。
- **4cbd 来源函数**：`game/action.rs::{step_blasters, remove_bot, step_self_destruct, eval_tick}`。
- **残余问题**：尚未同时绑定矿槽释放断言；引擎内部已有释放逻辑，本轮外部回归聚焦重复移除。

### R18 终局边界

- **CODE 结论**：默认配置在 `9000 - 3000 = 6000` 进入终局；tick 5999 仍可生产，`eval_tick` 后递增到 6000；tick 6000 起自然和加急生产均停止且不扣款。进入终局后空舰队触发 elimination 判胜。
- **DOC 描述**：5999 仍处于可生产阶段，递增到 6000 后进入终局；6000 起停止造兵并启用相应判胜边界。
- **测试函数/命令**：`r18_tick_5999_builds_then_6000_enters_endgame_and_stops_building`；C1。
- **实际结果**：**PASS**。5999 的到期生产成功，tick 变为 6000 后空对手按 elimination 判负；6000 的到期加急请求不生产、不扣 50 token。
- **4cbd 来源函数**：`game/state.rs::GameState::in_endgame`；`game/action.rs::{step_fabricators, eval_tick, match_result}`。
- **残余问题**：终局内持续载具移动、目标到达和最终 tick 9000 的组合行为未在本条穷举。

### R19 决胜顺序

- **CODE 结论**：时间到达后依次比较载具进度、双方存活单位总生命、token；全部相等才平局。任意可表示的正/负非零进度都优先于生命和资金。
- **DOC 描述**：进度优先，其后血量，再资金；近零正负进度分别验证。
- **测试函数/命令**：`r19_tiebreak_order_is_progress_then_health_then_tokens_with_near_zero_sign`；C1。
- **实际结果**：**PASS**。`+f32::EPSILON` 判 A、`-f32::EPSILON` 判 B；进度归零后生命覆盖资金，生命相等后资金决定，全部相等为 Tie。
- **4cbd 来源函数**：`game/action.rs::{match_result, tiebreak}`；`game/state.rs::GameState::health_pool`。
- **残余问题**：代码使用 `f32::total_cmp`，没有文档层的近零容差；是否应把微小数值视为零需组织方确认，当前 CODE 是严格符号比较。

### R20 官方镜像

- **CODE 结论**：一次镜像旋转位置/速度/动作坐标、交换两队舰队/矿点/生产器、翻转 capture 符号和矿点 Team 标签；二次镜像对 `GameState` 与 `FleetAction` 均恢复原值。
- **DOC 描述**：官方镜像把己方归一化到同一视角；状态/动作往返、进度符号和标签变换必须正确，两个 team 应使用同一正式策略。
- **测试函数/命令**：`r20_state_and_action_mirror_round_trip_flips_progress_positions_and_labels`；C1。策略两队同构另由 `both_team_slots_construct_the_same_controller_state_machine`；C2。
- **实际结果**：**PASS**。验证 capture `0.25 → -0.25`、位置绕 32×32 中心旋转、矿点占用标签 A↔B、token/舰队交换及状态/动作双镜像恒等；两个 team controller 初始状态机一致。
- **4cbd 来源函数**：`game/config.rs::{mirror_pos, mirror_vel}`；`game/state.rs` 中 `Mirror for {BotState, SpecialState, FleetAction, Deposit, GameState}`。
- **残余问题**：本合同验证数据变换和控制器同构，尚未以完整双方向比赛证明长期策略轨迹完全对称；随机顺序仍可能造成样本差异。

### R24 终局采矿差异：CODE / DOC / OPEN

- **CODE 结论**：**终局仍会执行采矿并把收入记入生产器 token。** `step_fabricators` 在终局提前返回，只停止生产；`eval_tick` 随后仍无条件调用 `step_extractors`。
- **DOC 描述**：Wiki 对终局收益的文字可被读作终局不再获得该收益，与当前代码执行结果不一致。
- **测试函数/命令**：`r24_endgame_still_credits_mining_code_doc_open_contract`；C1。
- **实际结果**：**PASS（CODE 行为）**。tick 6000 的 Extractor 正常占槽，token 从 0 增至 0.05；同时没有因到期计时器额外生产单位。
- **4cbd 来源函数**：`game/action.rs::{step_fabricators, step_extractors, eval_tick}`；`game/state.rs::GameState::in_endgame`。
- **OPEN / 残余问题**：本测试只证明 `4cbd3467` 的可执行事实，**不裁决 Wiki 还是代码代表组织方最终意图**。需组织方确认后选择修正文档或引擎；在确认前，实验和策略按 CODE 行为记录，报告不得把争议写成已解决规则。

### R26 v0 合法动作

- **CODE 结论**：当前 v0 在空舰队及含 Battle、Healer、Extractor 的代表状态中返回有限移动向量、与实际单位类别一致的特殊动作、关闭自毁，并保证激活的治疗目标不是自身且仍存在。tick 0 空舰队选择 Extractor 并等待免费自然生产，不错误加急。
- **DOC 描述**：v0 必须输出有限数值、正确类别、合法目标，能处理双方入口、空舰队、死亡和目标失效。
- **测试函数/命令**：`strategy::controller::tests::r26_empty_fleet_and_active_classes_produce_legal_finite_actions`；C2。双方入口同构由 `strategy::controller::tests::both_team_slots_construct_the_same_controller_state_machine` 同批通过。
- **实际结果**：**PASS**。bot 测试批次共 4 项全部通过；R26 的空舰队和三类别代表单位断言全部成立。
- **4cbd 来源函数/接口**：`game/state.rs::{FleetAction::new, SpecialAction::class, BotArray::get}` 定义合法动作结构与目标槽位；实际策略来源为 `bot/src/strategy/controller.rs::Controller::decide` 及 `bot/src/strategy/{economy,tactics,geometry}.rs`。
- **残余问题**：单元测试不等同完整比赛证明；尚需用正式计时 smoke 覆盖单位死亡后的目标失效、全部 32 个槽位、低预算分支和长期数值稳定性。动作合法性测试也不证明动作具有竞技强度。

## 3. 规则合同总览

| 范围 | 结果 | 说明 |
|---|---:|---|
| 独立 engine 合同 | 17 PASS | R01、R02、R03、R04、R05、R06、R07、R08、R09、R10、R13、R16、R17、R18、R19、R20、R24 |
| bot 单元测试 | 4 PASS | 包含 R26、两队 controller 同构、生产 4/9/4、射线圆相交辅助 |
| 失败 / 忽略 | 0 / 0 | 两个测试批次均无失败和忽略项 |
| 文档冲突 | 1 OPEN | R24 终局采矿收益 |

这些测试锁定的是提交 `4cbd3467…` 与当前 v0 的已执行行为。上游提交、配置 fixture 或策略实现变化后必须重新运行，不能把本轮 PASS 自动外推到新版本。

## 4. 后续测试积压

1. **移动与转向细节**：逐项锁定移动后转向、`TargetRotation`/`TargetPosition`、角度归一化、最大转速、地图边界和墙碰撞顺序。
2. **动态实体避让**：覆盖载具和矿点圆形实体、移动目标、重叠单位及导航辅助只感知静态地图时的局部控制行为。
3. **BotId 复用**：验证单位死亡、槽位复用、旧动作和跨 tick 缓存不会错误绑定到新单位，并覆盖采矿占用位图清理。
4. **跳 tick 恢复**：模拟预算跳过和不连续 `state.tick`，验证过期目标、火力登记、任务缓存及遥测对齐被正确清理。
5. **载具路径常量耦合**：检查 `GameConfig.payload_path` 与 `payload_position`/`payload_path_len` 使用的编译期路径常量是否可能漂移，并为非默认路径配置建立最小复现。
6. **终局进度锁定**：覆盖进入终局后的载具继续移动、到达目标、消灭判胜与 max tick 决胜之间的优先级，明确进度是否及何时锁定。
