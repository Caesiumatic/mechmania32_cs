# mechmania32_cs

MechMania 32 的可复跑 Rust bot 与规则实验仓库。当前 M1 建立了官方 CLI 工程、固定上游规则合同、v0 策略、正式计时 smoke 工具和结构化证据。

## 目录

- `bot/`：官方 Rust starterpack；参赛源码仅位于 `bot/src/strategy/`。
- `lab/rules/`：固定官方 engine 提交的独立合同测试。
- `lab/opponents/`：可追溯的 smoke 对手策略源。
- `configs/`：上游锁、v0 参数和 8 局 smoke 矩阵。
- `tools/mm32.py`：统一的诊断、初始化、测试、比赛、审计与打包入口。
- `docs/`：原始任务、计划、规则审计和本轮报告。
- `artifacts/`：精简机器指标；大日志只保存在 Git 忽略的 `.local/`。

## 快速开始

```bash
python3 tools/mm32.py doctor
python3 tools/mm32.py bootstrap
python3 tools/mm32.py test
python3 tools/mm32.py smoke
python3 tools/mm32.py packcheck
python3 tools/mm32.py audit
```

工具链默认从项目私有 `.local/` 读取，不污染全局 Cargo/Rustup 状态。精确上游版本和已知 client/runtime 差异见 `configs/upstream.lock.json`。

## 开发流程

