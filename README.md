# DOKP — Dormitory Observation & Knowledge Platform

## Document Map

| Document | Role | Status |
|---|---|---|
| [plan.md](plan.md) | 親仕様。システム全体の概念・構造・要件を定義する | Active — 正典 |
| [domain_algebra.md](domain_algebra.md) | 型定義・law・失敗モデル・write 正規化・storage 意味境界 | Active — plan.md の意味論補強 |
| [governance_capability_model.md](governance_capability_model.md) | consent・access・agent capability・write review・retention | Active — plan.md §7 の拡張 |
| [runtime_reference_architecture.md](runtime_reference_architecture.md) | runtime topology・技術マッピング・運用制御 | Active — 交換可能な参照実装 |
| [adr_backlog.md](adr_backlog.md) | 未確定の設計判断を追跡する backlog | Active — 継続更新 |
| [open_issues.md](open_issues.md) | 横断的な論点整理と具体的な提案 | Active — 次の設計ラウンド用 |

### Archive

| Document | 元の役割 | アーカイブ理由 |
|---|---|---|
| [archive/design_questions.md](archive/design_questions.md) | 追加設計論点の Q&A シート | 回答済み。成果は domain_algebra / governance / adr_backlog に反映済み |
| [archive/plan_refinement_functional_architecture.md](archive/plan_refinement_functional_architecture.md) | 関数型アーキテクチャ観点の洗練メモ | 提案は domain_algebra / runtime に反映済み |
| [archive/open_issues_round1.md](archive/open_issues_round1.md) | Round 1 の横断的論点整理と提案 | 回答済み。成果は各仕様文書と adr_backlog に反映済み |

## Reading Order

1. **plan.md** — まず全体像を掴む
2. **domain_algebra.md** — 型と law の厳密な定義を確認する
3. **governance_capability_model.md** — consent・権限・agent の扱いを確認する
4. **runtime_reference_architecture.md** — 実装に落とすときの参照構成を見る
5. **adr_backlog.md** — 何が未確定かを把握する
6. **open_issues.md** — 次の設計ラウンドで詰めるべき論点を確認する
