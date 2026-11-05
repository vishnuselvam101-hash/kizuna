# 用語集

日本語 UI・英語 UI での表記です。レポートや社内教育でも同じ用語を使ってください。

| 日本語（UI） | English (UI) | 意味 |
|---|---|---|
| 証拠 | Evidence | 文書と、そこから取り出したクレーム。 |
| クレーム | Claim | 1 つの出典が述べている 1 つの事実。識別子・属性・値・単位・リビジョン・出典の位置からなる。いわゆる「主張」に当たるが、UI では「クレーム」と表記する。 |
| 属性 | Attribute | クレームの対象（寸法、材質、承認など）。 |
| 出典 | Source | クレームの元になった文書と位置。 |
| 位置 | Source location | 出典内のページ、シート、セル、行、列、行番号。 |
| 手入力 | Manual entry | 出典文書なしで人が入力したクレーム。 |
| 信頼度 | Confidence | 抽出方法の確からしさ。値が正しい確率ではない。 |
| レビュー | Review | 理由を添えて記録する、人による判定。 |
| 未レビュー／承認済み／却下／証拠待ち | Unreviewed / Accepted / Rejected / Needs evidence | クレームのレビュー状態。 |
| 置換済み | Superseded | 新しい版に置き換えられた状態。旧版は保持される。 |
| 文書 | Document | アップロードされ、変更されずに保存されたファイル。 |
| 取込 | Capture | ファイルの 1 回のアップロード。同一ファイルは保存を共有するが、取込はそれぞれ記録される。 |
| 受理／隔離／拒否 | Accepted / Quarantined / Rejected | 文書のスキャン結果。 |
| 完全性 | Integrity | 保存バイトが取込時から変わっていないこと。 |
| クレーム抽出／再抽出 | Extract / Re-extract | 文書から未レビューのクレームを作ること／理由を付けてやり直すこと。 |
| レコード | Record | 部品またはサプライヤーと、そのクレームのまとまり。 |
| 部品／サプライヤー | Component / Supplier | レコード種別。 |
| 識別子 | Identifier | レコードを特定する部品番号やサプライヤー名。 |
| リビジョン | Revision | 文書や仕様の版。 |
| 矛盾 | Conflict | 証拠の中で見つかった食い違いや欠落。結論ではなく、レビューのきっかけ。 |
| 検証を実行 | Run verification | 現行のクレームに対して矛盾検出をやり直すこと。 |
| 特例として承認 | Approve (documented waiver) | 矛盾を、記録付きの例外として受け入れること。 |
| 証拠を却下 | Reject evidence | 矛盾の原因となった証拠を誤りとすること。 |
| 追加証拠を依頼 | Request more evidence | 追加の証拠が届くまで矛盾を保留すること。 |
| 対象外 | No longer applies | 証拠の変化により解消した矛盾。 |
| 判断 | Decision | 依拠したクレームとともに記録された結果。 |
| 根拠 | Rationale | 判断の理由を記した文章。 |
| 導出ハッシュ | Derivation hash | 判断とその証拠全体に対するハッシュ。 |
| 有効／影響の可能性／置換済み／取消済み | Valid / Potentially affected / Superseded / Revoked | 判断の状態。 |
| 影響を受ける判断 | Affected decisions | 判断後に証拠が変化した判断。 |
| 再確認 | Reconfirm | 現在の証拠に基づき、判断の新しい版を記録すること。 |
| 取り消す（取消） | Revoke | 判断を確定的に無効にすること。元に戻せない。 |
| 上流の判断 | Upstream decision | ある判断が依拠している別の判断。 |
| 重要／軽微 | Material / Not material | 変化が判断にとって重要かどうか。 |
| 監査証跡 | Audit trail | 記録されたすべての操作の一覧。 |
| 監査チェーン／ハッシュチェーン | Audit chain / Hash chain | 各監査イベントを直前のイベントと連結し、改変を検知できるようにする仕組み。 |
| 改ざんなし／破損 | Intact / Broken | チェーン検証の結果。 |
| インシデント | Incident | 検知ルールに該当したセキュリティイベントのパターン。 |
| 封じ込め済み | Contained | 自動対応が実行されたインシデント。 |
| 誤検知 | False positive | 実際の脅威ではなかったインシデント。 |
| 検知しきい値 | Detection thresholds | インシデントを発生させるイベント件数。 |
| 取込設定 | Import settings | CSV・XLSX・PDF の読み取り方の設定。 |
| AI 支援抽出 | AI-assisted extraction | 外部 AI プロバイダーによる、任意のラベル対応付け。 |
| 認証アプリ | Authenticator | サインイン用コード（TOTP）を表示するアプリ。 |
| 組織 | Organisation | KIZUNA 内で会社ごとに分離されたデータ領域。 |
| 管理者／レビュアー／アナリスト／監査担当／閲覧者 | Administrator / Reviewer / Analyst / Auditor / Viewer | ロール。 |

UI に表示されない用語：
| 日本語 | English | 意味 |
|---|---|---|
| 4つの目の原則（ダブルチェック） | Four-eyes principle | 自分の作業を自分でレビュー・解除しないこと。 |
| セキュリティキー | Security key | WebAuthn による物理的な第 2 要素（API のみ）。 |
| リカバリーコード | Recovery code | セキュリティキー紛失時に使う使い捨てコード。 |
| 運用者 | Operator | サーバー、バックアップ、シークレットを管理する人。 |
| 是正処置（8D） | Corrective-action case | 指摘から起票し、D1〜D8 の各ステップを引用した証拠で完了する（ADR 0024）。 |
| 暫定処置 | Containment | D3: 不良品の流出を直ちに止める処置。 |
| 根本原因（発生原因・流出原因） | Root cause (occurrence / escape) | D4: なぜ起きたか、なぜ流出したか。各原因はクレームまたは文書を引用する。 |
| 恒久対策 | Permanent corrective action | D5: 担当者と期限を持ち、根本原因に対応する対策。期限超過は通知される。 |
| 効果確認 | Verification of effectiveness | D6: 対策後に記録された証拠（SPC 起点では管理状態への復帰）で示す。サプライヤー自身の報告だけでは不可。 |
| 水平展開 | Horizontal deployment | D7: 類似の製品・ライン・拠点への展開（不要ならその理由）。 |
