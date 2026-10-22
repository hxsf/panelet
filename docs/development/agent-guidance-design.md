# Agent 指令分层说明

初次整理：2026-09-07；Panelet 命名与目录更新：2026-10-22。本文是维护说明，不是每次任务的前置阅读清单。

## 资料与采用方式

- Eric Provencher：[Rethinking skills and prompts for GPT-6 Astra](https://x.com/pvncher/status/2095991462416490862)。已通过浏览器读取作者原文。采用其按需披露、精简常驻指令、明确完成边界的思路；不把旧模型所需的逐步流程固化为所有任务的必经步骤。
- OpenAI：[Using GPT-6 Astra](https://developers.openai.com/api/docs/guides/latest-model#prompting-best-practices)。采用持续推进、澄清实际阻塞、说明 skill 引发的暂停、简洁表达、按风险验证等建议。多代理按任务收益与运行环境约束选择，不照搬“能拆就拆”的示例；本文不改变模型、推理强度或 API 配置。
- OpenAI：[Custom instructions with AGENTS.md](https://learn.chatgpt.com/docs/agent-configuration/agents-md)。使用标准文件名 `AGENTS.md`。用户级文件位于 `CODEX_HOME`（本机为 `~/.codex`），项目级放仓库根目录；更近的项目指令细化通用偏好。`AGENTS.override.md` 可遮蔽同级文件，新会话重新发现指令。

## 内容归属

| 位置 | 保留内容 |
| --- | --- |
| 用户级 `~/.codex/AGENTS.md` | 中文沟通、任务完成、授权边界、按需阅读、skill 使用、适量验证与协作偏好 |
| 项目 `AGENTS.md` | Panelet 模块职责与验证边界、当前 NOTE4C 硬件安全、设备证据及接入入口 |
| README 与 `docs/` | 当前状态、详细操作、运行测量、协议契约与开发记录 |
| 任务提示 | 当次目标、验收结果、范围、特殊授权和预算 |

这是结合当前仓库作出的编辑判断，不是两篇文章提供的 NOTE4C 专用模板。旧指令中的供电、strapping、全刷默认、BUSY 超时、安全关屏、备份、共享 I2C、帧格式及证据规则均保留；补充 `cargo run` 会触发刷写这一项目边界。已存在的局刷实验只链接证据，不提升为默认能力。入口根据当前工作树迁移为 `app/os`，不回滚既有开发改动。

维护时，只把跨任务长期有效、能够改变决策的规则提升为常驻指令。偶发流程放 skill 或专题文档，入口简短且按任务触发；先删除重复、矛盾和过时规则，再考虑增加新规则。硬件安全约束的精简应保留其可执行条件。

验证本次修改只需核对路径、命令对应的实际文件、原有安全规则及 Markdown diff；不运行固件构建或连接开发板。新开 Codex 会话后，可要求它列出加载的指令来源以验证实际加载。
