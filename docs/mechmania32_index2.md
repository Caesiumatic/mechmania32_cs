# MechMania 32 M2 索引

## 任务与基线

- [M1 索引](mechmania32_index1.md)
- M2 被评估代码：`761c8f80b01fbbf5d50cd2e4b4b719d269e7ea08`
- 历史 v0 来源：`d13c2360e1ce5708078b45254fc862de8ce8341f`

任务原文已从本机附件逐字导入，SHA-256 为 `3e6979128bea47ec4dffcac4cb0f7fa2d54129d44dfe5819513bfaf0dbe3a02a`。任务中提到的 `mechmania32_review1.md` 未随本轮输入提供，因此没有伪造或导入该文件；任务原文中的必修发现作为实现范围。

## 实现、规则与报告

- [M2 执行报告](reports/mechmania32_report2.md)
- [M2 规则与策略合同](rules/mechmania32_rules2.md)
- 生产策略：`bot/src/strategy/`
- 精确射击查询：`bot/src/strategy/ballistics.rs`
- 治疗与火力协同：`bot/src/strategy/coordination.rs`
- 官方 oracle 与 D01-D05：`lab/rules/tests/policy_geometry.rs`
- 挑战对手与行为测试：`lab/opponents/`
- 统一执行入口与工具测试：`tools/mm32.py`、`tools/tests/test_mm32.py`

## 配置与机器证据

- `configs/toolchain.lock.json`
- `configs/suites/m02_regression.json`
- `configs/suites/m02_challenge.json`
- `configs/suites/m02_soak.json`
- `artifacts/summaries/m02/metrics.json`
- `artifacts/summaries/m02/manifest.json`
- `artifacts/summaries/m02/audit.json`
- `artifacts/summaries/m02/pre_fix_ballistics.json`
- `artifacts/feedback/latest.json`

规范批次是 `regression-761c8f8`、`challenge-761c8f8`、`soak-761c8f8` 与 `clean-761c8f8`；同目录中的 `4e66145` 批次只作为先前成功尝试保留，不参与最终结论。大型回放、日志、固定上游展开树和构建缓存位于 Git 忽略的 `.local/`，精简证据保存路径、哈希与结果。
