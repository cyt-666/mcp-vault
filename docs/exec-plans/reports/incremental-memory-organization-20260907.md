# 增量记忆归并与升级验收

## 结论与范围

2026-09-07，本地基线 `8f559de` 上完成 ADR-0031。运行路径改为来源变更 → 独立贡献 → 有界候选批判断 → 正式记忆安全归并。旧全库配对调度与默认句子改写退出生产运行。

本次没有提交或推送 Git，没有部署，也没有读取生产数据库或调用真实付费模型。以下模型验证使用本地 HTTP 替身，证明调用、归并边界和恢复机制，不代表真实模型的语义质量。

## 实现结果

- 迁移 0024 删除旧自动整理任务的全部状态，以及旧候选、判断、改写缓存和进度。旧文件操作日志先恢复；规范分组通过 Vault Core 从当前贡献重新建立，再运行新方案。
- 原笔记、显式记忆、当前来源贡献、来源暂停状态及规定的审计／修订恢复数据保留。不根据历史猜测恢复旧算法已经移除的来源内容。
- 新任务 `memory.organize` 只保存来源和贡献级工作，不生成全库两两组合。每条贡献最多 8 个最终候选，一次结构化请求比较完整命题；只允许完整等价归并。
- 来源变更与工作标记同事务提交。正式合并与完成检查点、合并计数同事务提交。中断恢复、来源并发变化、请求引用校验、缓存和 Vault 隔离均有回归覆盖。
- 已完成输入保持空闲。新增贡献、相关向量到达和明确重检只唤醒相关工作；模型配置变化不拆散既有分组。来源更新保留模型退避，立即执行可显式唤醒等待任务。
- 管理页面支持立即整理、暂停、继续及默认关闭的重检选项。进度按来源贡献统计，另列成功合并、实际模型调用和缓存命中。无待处理内容时返回完成状态，不留下虚假的等待状态。

## 验证记录

| 检查 | 结果 | 证据 |
|---|---|---|
| `cargo fmt --all --check`、`git diff --check` | 通过 | 工作树检查 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 通过 | `target/memory-organization-validation/clippy-final.log` |
| `cargo test --workspace --all-features` | 370 项通过，0 失败 | `target/memory-organization-validation/workspace-tests-final.log` |
| 最终空任务反馈和稳定输入入队回归 | 通过 | `admin-controls-final.log`、`idle-admission-final.log` |
| 前端 lint、测试、构建 | 37 项测试通过，TypeScript 与 Vite 构建通过 | `pnpm --dir frontend/admin lint/test/build`；本机执行使用 `CI=true` |
| 真实 Chrome + Admin HTTP | 立即执行／暂停／继续、迁移、编辑、删除／恢复来源、分页通过 | `target/memory-organization-validation/browser/result.json`、`organization-panel.png` |
| 真实 HTTP 冒烟 | OAuth、MCP、Origin、50 个并发 WebDAV PUT、修订前置条件、管理平面隔离通过 | `target/memory-organization-validation/http-smoke.log` |
| 官方 MCP 固定套件 | 2026-07-28 的 6 个默认场景通过既有 baseline，无新增豁免 | `target/memory-organization-validation/conformance-final.log` 和 `conformance/` |

工作区全量检查后，页面回读发现空任务状态仍显示等待，已修正并追加最终管理接口与稳定输入回归。复选框复用既有布局样式，经最终构建与浏览器回读确认。

### 规模与语义边界

- 305 条不同的合成记忆完成整理，实际执行 305 次批量判断；待处理贡献为 0，没有旧配对队列。
- 再新增 1 条重复事实，只增加 1 次判断；306 条来源贡献对应 305 条正式记忆。
- 新批处理流程通过现有 60 个标注关系样例，包括完整等价、包含、相关、不同和不确定。测试向量故意允许硬负例进入候选，防止仅靠候选过滤伪造语义正确率。
- 两个 Vault 各 10,000 条旧候选的 schema-23 升级清理通过；旧自动任务删除，其他任务、操作恢复日志和身份保留记录不受影响。
- 覆盖已有组向量最后到达、来源在请求中途变化、合并规范写入中断、暂停跨重启、模型切换不重算、手动重检命中缓存和新增来源不绕过退避。

详细结果：`target/memory-organization-validation/scale.json`、`batch-corpus.json`。

## 环境问题与处理

首次沙箱运行出现以下限制，已通过获准的本机测试权限继续，最终检查没有因此跳过：

```text
Operation not permitted                 # 模型替身的本机端口绑定
ERR_PNPM_META_FETCH_FAIL                 # pnpm 依赖状态检查的沙箱网络限制
ModuleNotFoundError: No module named 'playwright'
Failed to resolve 'pypi.org'             # 临时测试虚拟环境下载依赖
Could not resolve host: github.com      # 官方套件固定源码下载
```

Playwright 1.55.0 安装在临时虚拟环境，浏览器使用本机 Chrome。没有安装或更改生产依赖。

官方套件的默认 `npm --prefix <checkout> ci` 在本机 npm 中报告：

```text
npm ci can only install packages when package.json and package-lock.json are in sync
Missing: official-conformance@0.2.0-alpha.11 from lock file
```

改为在固定源码目录执行 `npm ci --ignore-scripts` 和 `npm run build` 后通过。核对 SHA 为 `74edef34d674f563537be8c6587cebaa58e830ca`，`package.json` 和 `package-lock.json` 没有修改。随后以 `MCP_VAULT_CONFORMANCE_PACKAGE=file:<固定源码目录>`、`npm_config_offline=true` 执行原项目脚本。套件保留原有 prompts 等未声明能力的 baseline，不将预期失败写成全部断言通过。

历史迁移测试原来固定断言版本 23，并把 migration-22/23 的保留语义套用到所有后续迁移。本次将最新版本断言更新为 24；历史行为测试仅迁移到其目标版本，新迁移清理由独立升级测试覆盖。

## 运行与回滚

部署后的初始化与语义超时修复、迁移 0025 和最新 378 项 Rust／38 项前端测试结果，见[后续故障修复验收](organization-incident-fixes-20260907.md)。本报告前述数字保留为初始增量实现的验收记录。

升级后自动清理和重新整理，不需要点击迁移、重新提取笔记或重新绑定模型。回滚需要一起恢复升级前数据库与 Vault 备份；已删除的旧派生队列与判断缓存不逆向导入。真实模型质量和生产部署效果仍属于实际运行验收。
