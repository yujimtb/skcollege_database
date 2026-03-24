# DOKP — Dormitory Observation & Knowledge Platform

公開用リポジトリとして維持する前提で、機密情報とローカル実行データは Git 管理対象から外しています。運用上の扱いは [SECURITY.md](SECURITY.md) を参照してください。

## Document Map

### Specifications

| Document | Role | Status |
|---|---|---|
| [plan.md](plan.md) | 親仕様。システム全体の概念・構造・要件を定義する | Active — 正典 |
| [openspec/specs/_index.md](openspec/specs/_index.md) | モジュール仕様索引。M01-M15、依存 DAG、開発レーンを定義する | Active — 実装/検証の起点 |
| [domain_algebra.md](domain_algebra.md) | 型定義・law・失敗モデル・write 正規化・storage 意味境界 | Active — plan.md の意味論補強 |
| [governance_capability_model.md](governance_capability_model.md) | consent・access・agent capability・write review・retention | Active — plan.md §7 の拡張 |
| [runtime_reference_architecture.md](runtime_reference_architecture.md) | runtime topology・技術マッピング・運用制御 | Active — 交換可能な参照実装 |
| [adr_backlog.md](adr_backlog.md) | 未確定の設計判断を追跡する backlog | Active — 継続更新 |

### Issues

| Document | Role | Status |
|---|---|---|
| [open_issues.md](open_issues.md) | Issue インデックス（各ラウンドの概要） | Active |
| [issues/README.md](issues/README.md) | Round 2 Issue 一覧・優先順位・担当エージェント | Active |
| [issues/R2-01〜R2-08](issues/) | 個別 Issue ファイル | Active |

### Development

| Document | Role | Status |
|---|---|---|
| [dev_advice.md](dev_advice.md) | 開発方針・AI と人間の役割分担 | Reference |
| [agents/README.md](agents/README.md) | マルチエージェント開発体制の概要 | Active |
| [agents/spec-designer.md](agents/spec-designer.md) | Spec Designer エージェント定義 | Active |
| [agents/implementer.md](agents/implementer.md) | Implementer エージェント定義 | Active |
| [agents/reviewer.md](agents/reviewer.md) | Reviewer エージェント定義 | Active |

### Archive

| Document | 元の役割 | アーカイブ理由 |
|---|---|---|
| [archive/design_questions.md](archive/design_questions.md) | 追加設計論点の Q&A シート | 回答済み。成果は domain_algebra / governance / adr_backlog に反映済み |
| [archive/plan_refinement_functional_architecture.md](archive/plan_refinement_functional_architecture.md) | 関数型アーキテクチャ観点の洗練メモ | 提案は domain_algebra / runtime に反映済み |
| [archive/open_issues_round1.md](archive/open_issues_round1.md) | Round 1 の横断的論点整理と提案 | 回答済み。成果は各仕様文書と adr_backlog に反映済み |
| [archive/open_issues_round2.md](archive/open_issues_round2.md) | Round 2 の統合版 Issue | 個別ファイルに分割済み |

## Current Implementation Snapshot

このリポジトリの現行コードは、仕様群の MVP 垂直スライスとコア意味論を **Rust crate** として検証する参照実装です。  
`plan.md` と `runtime_reference_architecture.md` の技術マッピングは参考構成であり、このリポジトリの実装言語やライブラリ選定を拘束するものではありません。

| Scope | Status | Evidence |
|---|---|---|
| M01-M06 Domain / Registry / Lake / Supplemental / Projection / Propagation | Implemented | `src/domain`, `src/registry`, `src/lake`, `src/supplemental`, `src/projection`, `src/propagation` |
| M08 Governance | MVP 最小実装 | `src/governance` (`PolicyEngine`, `AuditLog`, `FilteringGate`) |
| M09-M14 Adapters / Identity / Person Page / API | Implemented | `src/adapter`, `src/identity`, `src/person_page`, `src/api` |
| M15 Runtime | MVP 最小実装 | `src/runtime` (`LocalBuildRunner`, `config`, `health`, `heartbeat`) |
| M07 Write-Back | Post-MVP / 未実装 | `openspec/specs/write-back.md`, `src/domain/command.rs` |

### Verification

- `cargo build`
- `cargo test`
- 2026-03-10 時点で self-host binary と API integration test を含めて `cargo test` が通過

### Current Follow-Ups

- `M07 Write-Back` は仕様化済みだが、現行コードでは `Command` / `EffectPlan` の定義までで、write router や source-native write adapter は未実装
- `M08 Governance` は最小実装で、`lake::IngestionGate` の policy 呼び出しは今後の接続ポイントとして残っている
- `M15 Runtime` は local build runner ベースで、container sandbox は Growth 以降の扱い

## Self-Host Quickstart

このリポジトリには、Slack と Google Slides をローカルで取り込み、person page API を返す self-host 用 binary が追加されています。

### Prerequisites

- Rust stable toolchain
- Slack Bot Token
- Google Slides / Drive を読める OAuth access token、または `client_id` / `client_secret` / `refresh_token`

### Configuration

1. `.env.example` を参考に `.env` を作る
2. 最低限、以下を設定する

`.env`、OAuth client JSON、SQLite、blob directory はローカル専用です。公開リポジトリには含めません。

- `DOKP_SLACK_BOT_TOKEN`
- `DOKP_SLACK_CHANNEL_IDS`
- `DOKP_GOOGLE_PRESENTATION_IDS`
- `DOKP_GOOGLE_ACCESS_TOKEN`

access token を毎回手で入れたくない場合は、代わりに以下を設定します。

- `DOKP_GOOGLE_CLIENT_ID`
- `DOKP_GOOGLE_CLIENT_SECRET`
- `DOKP_GOOGLE_REFRESH_TOKEN`

Notion への write-back も確認したい場合は、以下も設定します。

- `DOKP_NOTION_TOKEN`
- `DOKP_NOTION_DATABASE_ID`

Google Slides の AI 抽出を有効にする場合は、以下も設定します。

- `DOKP_GEMINI_API_KEY`
- `DOKP_GEMINI_MODEL` (`gemini-2.5-flash` 既定)

Notion database 側には title property が最低限 1 つ必要です。`Email` property は必須ではありませんが、ページ照合の安定性のため強く推奨します。

現在の adapter は、存在する場合に以下の database property も同期します。

- `Birthplace` (rich text)
- `DoB` (rich text)
- `Hashtag` (rich text)
- `Major_Interests` (rich text)

プロフィール本文、画像、ギャラリー、narrative sections は database property ではなく page body block として描画されます。

### Run

```bash
cargo run --bin dokp-selfhost
```

起動後の主な endpoint:

- `GET /health`
- `POST /admin/sync`
- `GET /api/persons`
- `GET /api/persons/{person_id}`
- `GET /api/persons/{person_id}/slides`
- `GET /api/persons/{person_id}/messages`
- `GET /api/persons/{person_id}/timeline`

### Notes

- 永続化は SQLite + ローカル blob directory を使います
- 既定の runtime state は `./data/` 配下に作られ、このディレクトリは Git では無視されます
- SQLite にある観測が重複していた場合、bootstrap は黙って捨てずにエラーとして扱います
- API は internal-only 前提で、現状は簡易構成のため認証を入れていません
- person detail では `Filtering-before-Exposure` により `identities` を非表示にしています
- identity / person-page の時刻は壁時計ではなく入力観測・補助レコードから導出し、replay の決定性を保ちます
- slide-analysis と Notion write-back の失敗は同期中に握り潰さず、その場で返します
- 秘密鍵・アクセストークンを一度でもローカルで使った場合は、公開前に新しい値へローテーションしてください

## Reading Order

1. **plan.md** — まず全体像を掴む
2. **domain_algebra.md** — 型と law の厳密な定義を確認する
3. **governance_capability_model.md** — consent・権限・agent の扱いを確認する
4. **runtime_reference_architecture.md** — 実装に落とすときの参照構成を見る
5. **adr_backlog.md** — 何が未確定かを把握する
6. **open_issues.md** → **issues/** — 次の設計ラウンドで詰めるべき論点を確認する
7. **agents/** — マルチエージェント開発体制と各ロールの定義を確認する
