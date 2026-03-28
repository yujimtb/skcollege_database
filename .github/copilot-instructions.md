# LETHE — Copilot Project Instructions

## Project Overview

Dormitory Observation & Knowledge Platform (LETHE): 学生寮に関わるあらゆる「観測（Observation）」を普遍的に蓄積し、そこから誰でも自由にデータベースを構築・合成できる開放的な知識基盤。

## Document Map

| Document | Role |
|---|---|
| `plan.md` | 親仕様（Authoritative overview） |
| `domain_algebra.md` | 型・law・failure model（Normative semantic companion） |
| `governance_capability_model.md` | consent・access・capability（Normative policy companion） |
| `runtime_reference_architecture.md` | runtime topology（Reference implementation） |
| `adr_backlog.md` | 未確定論点 backlog |
| `open_issues.md` | Issue インデックス → `issues/` に個別ファイル |
| `dev_advice.md` | 開発方針・役割分担の参照文書 |

## System Laws（絶対に破ってはならない）

| Law | Meaning |
|---|---|
| **Append-Only Law** | Canonical Observation を破壊的更新しない |
| **Replay Law** | pin された同一入力から同一 Projection 結果を得る |
| **Effect Isolation Law** | ドメイン解釈は hidden mutable state に依存しない |
| **Explicit Authority Law** | すべての write は authority model で正当化する |
| **No Direct Mutation Law** | Projection materialization を正史として更新しない |
| **Filtering-before-Exposure Law** | restricted data は表示・配布前に filtering projection を通す |

## Architecture Layers

| Layer | Responsibility |
|---|---|
| **Domain Kernel** | Observation / Projection / Command / Lineage の意味論 |
| **Policy Layer** | consent, access, review, retention, approval |
| **Effect Ports** | blob save, source read, source-native write, DB materialize |
| **Adapters** | Google, Slack, Figma, sensor, storage, API adapters |
| **Runtime / Scheduler** | crawl, build, replay, refresh, queue |

## Agent Roles

このプロジェクトは3つのエージェントロールで開発を進める。詳細は `agents/` を参照。

| Role | File | 責務 |
|---|---|---|
| Spec Designer | `agents/spec-designer.md` | interface / invariant / test plan / schema 設計 |
| Implementer | `agents/implementer.md` | コード実装・テスト・migration |
| Reviewer | `agents/reviewer.md` | System Law 違反・意味論逸脱の検出 |

## Coding Conventions

- Task 粒度: 半日〜2日で完結する単位に分割する
- 1PR 1意味: 複数の意味変更を1つの PR に含めない
- 仕様変更と実装変更は別の PR にする
- AI 生成コードは必ず test 付きで受け取る
- 実装コード / 単体テスト / integration test / API 仕様変更 / リスクメモ をセットで出す

## MVP Scope

**Google Slides + Slack → 名寄せ → 個人ページ** の1本の垂直スライス。

### MVP で作るもの
- Source adapter (Slack, Google Slides)
- 名寄せ Projection
- 個人ページ Projection / API
- Incremental propagation
- 最小 governance (internal-only + restricted + audit)

### MVP で作らないもの
- source-native write-back
- generic writable projection
- full multimodal OCR/caption/embedding
- すべての SaaS source への一般化
- DOI 自動化
- 完全な consent lifecycle
- agent sandbox 本番運用
- GUI 作り込み
