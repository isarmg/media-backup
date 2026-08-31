# Media Backup 文档总览

本文档集描述仓库当前 `0.2.0` 代码。代码、Schema、发行 manifest 与自动化测试是最终事实源；文档与
实现不一致时，应先确认当前代码，再在同一变更中修正文档和测试。

| 分类 | 入口 | 适合读者 | 解决的问题 |
|---|---|---|---|
| 初学者学习指南 | [beginner-guide/README.md](beginner-guide/README.md) | 第一次接触 Rust 或移动备份的开发者 | 如何建立心智模型、运行项目并安全修改代码 |
| 工作流程与流程树 | [project-workflow.md](project-workflow.md) | 开发、评审和排障人员 | 请求、上传、同步、恢复、发行如何流转 |
| 完整功能与取舍 | [feature-inventory-and-tradeoffs.md](feature-inventory-and-tradeoffs.md) | 产品、架构和维护人员 | 已实现什么、明确不做什么、为什么 |
| 必要 README | [../README.md](../README.md) | 所有人 | 项目定位、入口、最短验证路径 |
| 运维 | [operations.md](operations.md) | 部署、值班和发布人员 | 配置、安装、健康检查、故障处理和备份边界 |

阅读建议：初学者按表格从上到下阅读；处理线上问题时直接从运维文档的“故障定位顺序”开始；修改
协议、Schema 或发行布局前，必须同时阅读工作流程与功能取舍清单。
