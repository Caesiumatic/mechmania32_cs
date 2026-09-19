# MechMania 32 M1 索引

## 任务与计划


两份原文均从本机可读附件逐字导入；SHA-256 由 `artifacts/summaries/m01/manifest.json` 记录。

## 规则与实现

- [规则合同报告](rules/mechmania32_rules1.md)
- [M1 执行报告](reports/mechmania32_report1.md)
- v0 策略：`bot/src/strategy/`
- 独立合同工程：`lab/rules/`
- 统一入口：`tools/mm32.py`

## 机器证据

- `configs/upstream.lock.json`
- `configs/baseline.json`
- `configs/suites/smoke.json`
- `artifacts/summaries/m01/manifest.json`
- `artifacts/summaries/m01/metrics.json`
- `artifacts/summaries/m01/audit.json`
- `artifacts/feedback/latest.json`

原始比赛日志、临时对手工程和构建缓存位于 `.local/`，不进入 Git；摘要文件保存相对路径与 SHA-256。

