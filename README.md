# mechmania32_cs

MechMania 32 的可复跑 Rust bot 与规则实验仓库。当前 M2 在 M1 基础上加入与固定官方引擎一致的射击几何、两阶段治疗协同、固定历史 v0/挑战对手评估，以及经过执行的 tracked-only 冷恢复流程。

## 目录

- `bot/`：官方 Rust starterpack；参赛源码仅位于 `bot/src/strategy/`。
- `lab/rules/`：固定官方 engine 提交的独立合同与生产几何 oracle 测试。
- `lab/opponents/`：独立挑战对手、行为合同与弱基线策略源。
- `configs/`：来源锁、工具链选择以及 round/suite/run-id 对战矩阵。
- `tools/mm32.py`：统一的诊断、初始化、测试、比赛、审计与打包入口。
- `docs/`：原始任务、计划、规则审计和本轮报告。
- `artifacts/`：精简机器指标；大日志只保存在 Git 忽略的 `.local/`。

## 快速开始

```bash
python3 tools/mm32.py doctor
python3 tools/mm32.py bootstrap
python3 tools/mm32.py test --round m02 --suite configs/suites/m02_regression.json
python3 tools/mm32.py smoke --round m02 --suite configs/suites/m02_regression.json --run-id <唯一批次名>
python3 tools/mm32.py packcheck --round m02 --suite configs/suites/m02_regression.json
python3 tools/mm32.py audit --round m02 --suite configs/suites/m02_regression.json
```

工具链优先采用显式 `MM32_*` 配置，其次保留调用者已有的 Cargo/Rustup 安装；只有缺少完整调用者工具链时才回退到由 bootstrap 初始化的项目 `.local/`。实际编译器、target、平台和每个本机二进制都写入忽略目录中的构建记录；源码来源约束单独位于 `configs/upstream.lock.json`。

`cleancheck` 必须针对已提交的代码 ref 运行。它从 Git 跟踪内容创建独立副本，依次执行 bootstrap、测试、小规模正式计时比赛、packcheck 和第二次 bootstrap；固定来源归档与共享 Cargo/Rustup 缓存条件会显式记入 manifest。

## 开发流程

