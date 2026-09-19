# MechMania 32 M1 / round 1 实施报告

本报告只把实际执行结果写成“事实”；设计解释标成“推导”，策略前提标成“假设”，尚未执行或需外部裁决的事项标成“OPEN”。所有路径均相对于仓库根目录，原始大日志位于 Git 忽略的 `.local/`。

## 1. 本轮状态

**状态：`completed`（本地 M1 与 Git 交付）。**

本地工程目标已经完成：官方 Rust 工程可构建；两个 team 入口使用同一控制器；bootstrap 复跑保持策略哈希；17 个规则合同测试和 5 个 bot 单元测试全部通过；正式时间限制下完成 8/8 局；预算跳 tick 为 0；干净临时工程 packcheck 通过；精简指标、manifest、规则审计和反馈入口均已生成。

用户随后明确要求保留在 `main` 并 commit、push，因此没有创建或切换分支，也不创建 PR。实现、测试、配置和基础文档先形成代码提交 `d13c2360e1ce5708078b45254fc862de8ce8341f`；随后全部本地验证针对该提交重跑，manifest 与 audit 的 `evaluated_code_commit` 均绑定该 SHA。本报告和精简机器证据单独形成第二个证据提交；其 push 发生在 audit 之后，由 `origin/main` 与最终交付回执核验。

服务端正式提交验证也未执行。这是提示词明确保留给 Shi 单独授权的操作，不是本地 packcheck 失败。

## 2. 基线、版本和环境

### 2.1 仓库与提交语义

| 项目 | 实际值 | 结论 |
|---|---|---|
| repository | `Caesiumatic/mechmania32_cs` | remote 为 `https://github.com/Caesiumatic/mechmania32_cs.git` |
| 初始 / 交付 branch | `main` | 按用户明确要求保持在 `main`，未创建或切换分支 |
| base commit | `f02be38adce7fcf8e7dd5b5f8623c147bb8c449d` | 开始时工作树干净；原有 README、`.gitignore` 均保留并扩展 |
| evaluated code commit | `d13c2360e1ce5708078b45254fc862de8ce8341f` | 正式测试、8 局 smoke 与 packcheck 均在该代码提交后重跑并绑定此 SHA |
| delivery HEAD | 本报告所在证据提交 | Git 提交不能可靠地把自身 SHA 写入自身内容；确切 SHA 以 `origin/main` 和最终交付回执为准 |

### 2.2 冻结上游

执行开始时通过远端引用检查确认下列提交仍是对应仓库 HEAD；整个正式批次使用同一组冻结版本。

| 来源 | 仓库提交 | 本地归档 SHA256 |
|---|---|---|
| engine runtime | `4cbd3467e6617e330d42cb78ae3327ab2c88926a` | `7940b76b9365bf36501c23462ed1334ca43df8e05821f33f58160bd56f2332d7` |
| Wiki | `6af083bb4c28a4e18d2f7fcdda5a380aebc0545f` | `0e1260aee9951d2398bbfb8d3e0a3757d50b3570e8d5c40ea239b0af2972bc23` |
| rust-starterpack | `f9960353f414938002ea2bb18161b6851a71c40c` | `fdeb22694a86ea86e9c2460bcefe89916df17dfd0fc18751c0fe55289eb8f694` |
| CLI | `4c5bb2f33d7fdee10e9d8f311f382ff198b81214` | `3fcf97825033b37ad92049d0914e81760542f36842eb04e44dfaa71bee4fb1e3` |
| bot `mm-engine` client dependency | `879578d448251d0acc7f10eef3d7fe5450758096` | `bot/Cargo.lock` 实际解析值 |

事实：runtime `4cbd3467…` 比 client `879578d…` 多一个只修改 `src/game/action.rs` 的提交，修复“同 tick 被炮击击杀且请求自毁”的重复移除；`client` feature 不编译该文件。该差异已独立审计并记录为 accepted，而不是把握手成功当成规则等价证明。

| 二进制/配置 | SHA256 |
|---|---|
| 官方 `bot/.mm/bin/mm-engine` | `a07ca18f9d277f2c0d6b191edcb1eaebb0c02a10140283564b29ac5fe5d667a4` |
| 官方 `.local/bin/mm-cli` | `85daf9e00bc39dd45e530759328cd90210e6f857e34737c1bb61a27eecf92953` |
| 实际 GameConfig 首行 fixture | `ab2839fc9e2fa4214e694225120f3c7112ee587831f1bcb58cc6602bdf33dfc0` |
| `configs/baseline.json` | `22b5bc706306eba6f558c14260fe4a3307575cdbb3da12649ca3760d1be42110` |
| 参数集合 canonical hash | `sha256:51828cc65049dc403605cdb21b0314576cf3fd9fe18c4b6e9607ed9d563c9c85` |
| 当前策略源码树 | `9bbc438e5e66905e24ee5b3e1449c356cc4848351b6bfb2fbdac0e42e04ca4b8` |

### 2.3 系统与工具链

- 系统：Darwin `25.3.0`、arm64；Python `3.14.3`；Git `2.50.1 (Apple Git-155)`。
- 实际构建工具链：Rust `1.98.1` / Cargo `1.98.1` stable。
- 上游声明工具链：`nightly-2024-07-25`，实际为 Rust `1.82.0-nightly`。
- `mm-cli --version` 与 `mm-engine --version` 均为 `0.1.0`。
- OPEN/限制：当前依赖解析到需要 edition 2024 的 `clap_lex 1.1.1`，固定 nightly 的 Cargo 无法解析；因此全部成功构建和 packcheck 明确使用 stable，而没有伪称固定 nightly 成功。最小复现见第 8 节。
- 最终 doctor 中 `github.com:443` 因当前沙箱 DNS 不可达；所需固定归档、源码和二进制已在此前可联网阶段获取，并在最终 audit 中逐一重算哈希。


## 3. 已完成文件、职责和设计决定

### 3.1 目录边界

| 路径 | 职责 |
|---|---|
| `bot/src/strategy/` | 唯一生产策略源；官方 `Cargo.toml`、core、engine 和构建产物由官方工程生成并保持忽略 |
| `lab/rules/` | 独立 Cargo 项目，固定 engine `4cbd3467…` 并只启用 `engine` feature |
| `lab/opponents/` | 两个可版本化 smoke 对手的策略源 |
| `lab/fixtures/default_config.json` | 官方 engine 实际输出的完整 GameConfig；合同测试内部的小场景是合成隔离 fixture，二者不混写 |
| `configs/` | 上游锁、官方 `.mm` 锁模板、v0 参数与严格 2×2×2 smoke 矩阵 |
| `tools/mm32.py` | `doctor/bootstrap/test/smoke/packcheck/audit` 统一入口；明确退出码、超时、日志哈希、输入指纹和失败进度 |
| `docs/` | prompt、plan、规则审计、索引和本报告 |
| `artifacts/` | 可提交的精简 manifest、metrics、audit 和反馈入口 |
| `.local/` | 固定上游、工具链、target、临时对手、回放及原始日志；全部 Git 忽略 |

### 3.2 v0 设计

- 生产：参数化 `4 Extractor / 9 Battle / 4 Healer`。tick 0 优先免费自然生产；之后依据真实 token、人口和自然生产时刻加急；终局关闭生产。
- 经济：矿工使用配置数值、共享槽位状态、合法采矿距离、静态占用与 LOS 选择分散位置。
- 载具：维持一个有效 Battle controller；死亡时重选；使用合法控制环位置。
- 战斗：检查类别、射程、冷却、保护、转向、静态几何以及载具/矿点动态圆形阻挡；同 tick 对目标做有效命中去重。移动后 origin 预测精确镜像 `MoveAction::sanitize`：长度大于 1 的方向先归一化，短向量保留比例，避免把 `navigate_to` 长 delta 当成瞬移。
- 治疗：排除自己，只选受伤友军，并按运行配置限制每目标 assignment 数。
- 预算：每 tick 读取 `Budget`；低于 `100000` 时复用缓存目标，仍保留生产、简单移动、治疗和射击；跳 tick 时清理缓存。
- 遥测：内存累计，约每 250 tick 或异常时输出，析构时输出唯一 `MM32_SUMMARY`。
- 两个 team 都构造同一 `Controller`；镜像由官方 client/runtime 处理。
- 行为假设：敌方位置采用一 tick 匀速外推。这是 v0 启发式，不是规则事实，也没有被静态合同测试“证明”。
- 明确禁用 self-destruct；复杂火力预约、阵型、人口转换和搜索留给后续任务。

`starter_symmetric` 源自官方基础示例，但把原本单边入口统一为双方同一策略，并确保三类单位产生类别匹配动作。`rush_simple` 是项目内的零采矿 Battle rush，对载具控制环施压。完整临时工程和二进制只在 `.local/opponents/` / `.local/target/`。

## 4. 精确复跑命令与退出码

以下命令的 cwd 均记为脱敏占位符 `$REPO`（即仓库根目录）；参数已确认不含凭据。

| 命令 | 最终实际退出码 | 说明 |
|---|---:|---|
| `python3 tools/mm32.py doctor` | 0 | 本地工具、固定文件、版本均存在；网络探针为不可达但不是离线本地验证的失败条件 |
| `python3 tools/mm32.py bootstrap` | 0 | 固定归档与展开树一致；复跑前后策略树哈希均为 `9bbc43…`; 无文件需恢复 |
| `python3 tools/mm32.py test` | 0 | 包含 rustfmt、bot tests、rules tests、release build、`git diff --check` |
| `cargo +stable test --locked --manifest-path bot/Cargo.toml` | 0 | 5 passed / 0 failed / 0 ignored；含移动方向 sanitize 回归测试 |
| `cargo +stable test --locked --manifest-path lab/rules/Cargo.toml` | 0 | 17 passed / 0 failed / 0 ignored；另有 0-test lib/doc harness，不计为 skipped |
| `cargo +stable build --locked --manifest-path bot/Cargo.toml --release` | 0 | 候选 release 构建通过 |
| `rustfmt +stable --edition 2021 --check bot/src/strategy/*.rs lab/rules/src/*.rs lab/rules/tests/*.rs lab/opponents/*/*.rs` | 0 | strategy、规则、测试、对手均覆盖；manifest 记录展开后且以 `$REPO` 脱敏的 argv |
| `git diff --check` | 0 | tracked diff 通过；audit 另扫描 untracked 源文件空白 |
| `python3 tools/mm32.py smoke` | 0 | 串行、正式计时、严格 8 局全部有效 |
| `python3 tools/mm32.py packcheck` | 0 | 临时官方工程只恢复 strategy 后 stable release 构建通过 |
| `python3 tools/mm32.py audit` | 0 | `pass_with_warnings`；无 failures；两项 warning 分别为证据文件晚于代码提交、固定 nightly 的 toolchain override |

失败诊断没有混入 PASS：固定 nightly 的 bot test 和 CLI build 均真实退出 101，原始输出分别位于 `.local/diagnostics/nightly-bot-test.log`（SHA256 `c590a3609af72db852952d9b62bc972714241cc1ad38ae1ec403c616ac7ec24c`）和 `.local/diagnostics/nightly-cli-build.log`（`48aaa2d0fb89635767378a7331d77f6ba9ea90543c2c96aa914b1d788e3e61fa`）。

## 5. 规则合同覆盖、测试数字和文档差异

测试总数口径为：17 个 `lab/rules` engine 合同测试 + 5 个生产 bot 单元测试 = **22 passed / 0 failed / 0 skipped**。

| ID | 覆盖结论 | 实际结果 |
|---|---|---|
| R01 | 初态与 tick 0 免费生产 | PASS |
| R02 | 同 tick 加急/自然生产只新增一台并延期 | PASS |
| R03 | 32 人满人口持有并在释放后兑现 | PASS |
| R04 | 新 BotId 不读取旧槽位动作 | PASS |
| R05 | 治疗排除自己、范围与朝向边界 | PASS |
| R06 | 三份治疗封顶且治疗先于伤害 | PASS |
| R07 | 同 tick 保护、15 tick 到期、60 tick 冷却 | PASS |
| R08 | 同 tick 双方互相击杀仍都开火 | PASS |
| R09 | 射程端点爆炸与 10.55 中心距上界 | PASS |
| R10 | 首碰载具/矿点/敌军形成爆点且只伤敌方 | PASS |
| R13 | 单边一台/多台同速；双方争夺停驶 | PASS |
| R16 | 共享 8 槽、旧占用粘性、只补空位 | PASS |
| R17 | 炮击阵亡加自毁只移除一次 | PASS |
| R18 | 5999 可生产、递增至 6000 判胜并停产 | PASS |
| R19 | 进度、血量、资金决胜顺序及近零符号 | PASS |
| R20 | 官方状态/动作镜像往返 | PASS |
| R24 | 终局采矿行为差异 | PASS（合同确认 CODE，裁决仍 OPEN） |
| R26 | v0 空舰队、双方入口、类别和有限数动作 | PASS（生产 bot 单测） |

浮点比较统一使用绝对误差 `< 1e-4`。R16 不依赖随机新占用者的具体身份：断言 7 个 incumbents 保留、总槽位为 8、两个候选中新进者恰有一个获槽，并检查总收入；因此测试的是规则不变量而非 RNG 顺序。

R24 必须保持三分：**CODE**：engine 在终局仍执行采矿入账；**DOC**：冻结 Wiki 文字与此不一致；**OPEN**：等待组织方确认预期规则。未把该开放问题伪装成已裁决。详细函数、提交与残余问题见 `docs/rules/mechmania32_rules1.md`。这些是静态/合成 engine 合同；第 6 节的命中和预算是实际进程比赛数据，两类证据不互相替代。

## 6. 正式 smoke 结果和预算统计

### 6.1 批次定义与可追溯性

- 运行目录：`.local/runs/m01-smoke/run-20260919T040810Z`。
- 矩阵：2 对手 × candidate A/B 两方向 × 2 重复，严格各一次，共 8 局；串行，`time_enforcement=true`，单局外层超时 300 秒。
- engine 不提供 seed CLI，故 `engine_seed_controlled=false`、seed 为 `null`，重复局使用进程随机性。
- 所有比赛实际配置首行 SHA256 均为 `ab2839fc…`，与 fixture 相同。
- 二进制：candidate `05f173c382c123f3586a24dd13a3114f683c4c159a36ba9dada3a2e93f1c0130`；`starter_symmetric` `480c6f88f86e975751e7a660877d5ab41f2d6e1642aaedd3890206d30ac97f84`；`rush_simple` `47f4a2da669eca6092f19870d30464eabd0fc20ea091fd9c72df9d652413c86b`；engine `a07ca18f…`。
- 每局 exit 0、有完整结果行、官方错误日志无 fatal marker、唯一候选 summary 存在、callback 覆盖从 tick 0 到 `final_tick - 1`。全部因 payload 自然提前结束，无截断。

### 6.2 逐局结果

“里程碑”依次为首次到达载具 / 首次采矿 / 首次交火 tick；“费用”依次为 mean / p95 / p99 / peak；最后一列为调用数 / 有上一调用收费的观测数。

| match | 对手 | side | rep | 候选结果 | reason | final tick | wall s | exit | 里程碑 | 费用 | 调用/收费 |
|---|---|---:|---:|---|---|---:|---:|---:|---|---|---|
| `starter-symmetric-candidate-a-r1` | `starter_symmetric` | A | 1 | `win` | `payload` | 3754 | 1.003883 | 0 | 484 / 412 / 532 | 6.346 / 16 / 21 / 28 | 3754 / 3753 |
| `starter-symmetric-candidate-b-r1` | `starter_symmetric` | B | 1 | `win` | `payload` | 4047 | 0.371816 | 0 | 484 / 412 / 532 | 6.943 / 14 / 21 / 26 | 4047 / 4046 |
| `starter-symmetric-candidate-a-r2` | `starter_symmetric` | A | 2 | `win` | `payload` | 3754 | 0.332481 | 0 | 484 / 412 / 532 | 6.255 / 16 / 21 / 28 | 3754 / 3753 |
| `starter-symmetric-candidate-b-r2` | `starter_symmetric` | B | 2 | `win` | `payload` | 4047 | 0.380169 | 0 | 484 / 412 / 532 | 6.686 / 13 / 21 / 61 | 4047 / 4046 |
| `rush-simple-candidate-a-r1` | `rush_simple` | A | 1 | `win` | `payload` | 4052 | 0.498209 | 0 | 483 / 412 / 494 | 5.306 / 13 / 19 / 28 | 4052 / 4051 |
| `rush-simple-candidate-b-r1` | `rush_simple` | B | 1 | `win` | `payload` | 4052 | 0.296366 | 0 | 483 / 412 / 494 | 5.202 / 13 / 21 / 26 | 4052 / 4051 |
| `rush-simple-candidate-a-r2` | `rush_simple` | A | 2 | `win` | `payload` | 4052 | 0.279102 | 0 | 483 / 412 / 494 | 5.269 / 12 / 20 / 26 | 4052 / 4051 |
| `rush-simple-candidate-b-r2` | `rush_simple` | B | 2 | `win` | `payload` | 4052 | 0.294916 | 0 | 483 / 412 / 494 | 5.305 / 13 / 21 / 31 | 4052 / 4051 |

### 6.3 聚合解释

| 对手 | 样本 | 胜/平/负 | 3/1/0 总分 | 平均积分 |
|---|---:|---:|---:|---:|
| `starter_symmetric` | 4 | 4/0/0 | 12 | 3.0 |
| `rush_simple` | 4 | 4/0/0 | 12 | 3.0 |

预算聚合：31810 次 callback，31802 个收费观测（每局第一次调用没有“上一调用”收费，故正好少 8）；加权 mean `5.906420979812591`，各局最大 p95 `16`、最大 p99 `21`、全局 peak `61`，最低 remaining `500000`，low-budget tick `0`，跳 tick `0`。每局 `first_tick=0`、`last_tick=final_tick-1` 且 `calls=final_tick`，所以零跳 tick 有完整 callback 覆盖证据。

观测边界：遥测没有逐单位记录“末尾是否空动作”这一语义；它能确认末尾没有缺失 callback、没有引擎 fatal、没有预算跳过，且每个活跃 BotId 都由 controller 写入类别匹配动作。若下一轮需要逐单位空动作比例，应新增抽样计数，而不是从当前日志推断。一次探索性 self-play 位于 `.local/runs/manual-selfplay`，在 `metrics.json` 中明确标为不计入正式 8 局。

推导：8–0 只证明 v0 对两个弱、可追溯基线的可运行性和基本行为，不支持广泛强度、泛化或收敛结论。

## 7. 本地 packcheck 与 Git 审计

### 7.1 Packcheck

工具创建全新临时官方 rust-starterpack 工程，只用当前 `bot/src/strategy/` 替换其 strategy 目录，再执行 stable release build。源和复制后的策略树 SHA256 都为 `9bbc438e5e66905e24ee5b3e1449c356cc4848351b6bfb2fbdac0e42e04ca4b8`；退出码 0；产物 SHA256 `05f173c3…`，与正式候选相同。原始 build log 位于 `.local/records/logs/packcheck.log`，SHA256 `2f54e908293066d4fd0ffa927ed329134676ea399cb238f503fa9024c0d7ad62`。

结论严格限定为：**本地打包闭包通过（使用 stable 工具链）**。服务端提交/安装验证为 `not_run_requires_shi_authorization`。

### 7.2 Git 与证据审计

- `bootstrap` 对四个固定 tarball 重算 SHA，并比较实际展开树；能在缺失时下载精确归档、恢复官方 scaffold、CLI、engine 和 `.mm` 锁，同时不覆盖已有 strategy。本轮复跑没有发生恢复，策略前后哈希一致。
- 根 `.gitignore` 保留原有 macOS、IDE、环境、build/dist/coverage/log 规则，并新增 `.local/`、Python cache；官方 bot allowlist 只暴露 strategy、`mm-config.toml`、自身 `.gitignore`。
- `bot/Cargo.toml`、Cargo.lock、core、engine、target、临时工程、对手二进制、回放和大日志均为 ignored/generated；README、AGENTS、docs、configs、lab 和精简 artifacts 是候选文件。
- secret/token/带凭据 URL 扫描无发现；候选文件中无超过 1 MB 的文件；tracked 与 untracked 源文件空白检查通过；JSON 均可解析。
- audit 不再只信 `upstream.lock.json`：会读取当前 Cargo.lock、`.mm/lock.toml`，重算 CLI/engine/归档哈希，并把 test/metrics/manifest/packcheck 绑定到当前 strategy、rules、opponents、tool 和 config 输入指纹。每个子命令的最新 attempt 也必须成功，防止失败后误用旧 PASS。
- 最终 audit 状态：`pass_with_warnings`，`failures=[]`；warnings 为“仅报告/机器证据晚于已验证代码提交”以及“固定 nightly 与当前 edition2024 lock 不兼容”。前者正是两提交证据绑定设计，audit 已把 `evaluated_code_commit` 固定为 `d13c2360…`。

## 8. 失败、限制、最小复现和待确认项

### 8.1 Git 交付

- 用户明确要求保持在 `main` 并 commit、push；因此未创建或切换分支，也不创建 PR。
- 代码提交为 `d13c2360e1ce5708078b45254fc862de8ce8341f`；doctor、bootstrap、22 项测试、8 局 smoke、packcheck 和 audit 都在它之后重跑。
- 本报告与四份精简机器证据位于单独的证据提交。由于提交内容无法可靠自引用其最终 SHA，delivery HEAD 以 Git 历史、`origin/main` 和最终交付回执为准。

### 8.2 固定 nightly 与当前 lock 不兼容

- 最小复现 1：`cargo +nightly-2024-07-25 test --locked --manifest-path bot/Cargo.toml`，exit 101；`clap_lex 1.1.1` 要求尚未由该 Cargo 稳定支持的 edition2024。
- 最小复现 2：`cargo +nightly-2024-07-25 build --locked --release --manifest-path .local/upstream/cli/Cargo.toml`，exit 101；CLI manifest 自身要求 edition2024。
- 已完成部分：stable 的 test、release、8 局和 packcheck 全部通过；每份 manifest 都显式记录工具链 override。
- 最小恢复：由上游正式选择其一——更新 `rust-toolchain.toml` 到支持 edition2024 的工具链，或提供与旧 nightly 相容的依赖锁；之后重新运行全部证据链。未私自改写官方依赖以制造“nightly 通过”。

### 8.3 外部待确认

- 组织方 OPEN：R24 的终局采矿应以当前 CODE 行为还是冲突的 Wiki DOC 为准。
- Shi 授权：是否允许把本地通过的 package 提交到官方服务做 server-side validation。
- engine seed：当前 CLI 无 seed 参数，因此本批次不可精确重放 RNG，只能通过固定二进制、配置、矩阵和重复次数约束。
- 当前沙箱网络：最终 doctor 的 DNS 探针失败；本地固定资产验证不受影响，但从完全空缓存恢复 bootstrap 需要网络。

## 9. 下一轮最值得处理的三个问题

1. **统一官方工具链与依赖锁。** 证据是固定 nightly 两条最小复现均 exit 101，而 stable 全链通过。解决后可消除最重要的跨机器复现歧义，并把 bootstrap/CI 变成单一官方路径。
2. **补齐高风险规则与运行恢复合同。** 优先覆盖动态实体避让、BotId 复用、跳 tick 恢复、移动转向、载具路径常量耦合、终局进度锁定，并推动 R24 组织方裁决；收益是减少“静态合法但长局失效”的风险。
3. **建立更强对手与 M2 火力/阵型评测。** 当前 8–0 只来自两个弱基线。下一步加入可版本化的强/扰动对手、更多随机重复和命中/浪费统计，再实现火力预约、阵型与人口转换；只有这些证据才能支持强度结论。
