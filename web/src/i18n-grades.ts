// Error texts of the cross-standard grade guard API (tenant grade equivalences and limits, mfg-grade@1).
// Kept apart from i18n.tsx to keep merges small.
export const gradesEn = {
  "err.entry_id_required": "Choose the entry to change.",
  "err.entry_not_effective": "This entry has no approved version to change.",
  "err.four_eyes_required": "A second person must approve or reject this proposal.",
  "err.grade_equivalence_changed":
    "The grade equivalence this decision rested on was changed or retired. Re-evaluate the line and decide again.",
  "err.invalid_change": "The change must be create, edit or retire.",
  "err.invalid_note": "The note is too long or contains control characters.",
  "err.limit_exists": "An approved limit for this grade and property already exists. Edit it instead.",
  "err.no_equivalence_entry":
    "Your grade-equivalence table has no approved entry for this pair. Propose one (it needs a second person) first.",
  "err.no_open_substitution": "This certificate grade has no open substitution finding on this line.",
  "err.not_proposed": "This entry was already decided.",
  "err.pair_exists": "An approved entry for this grade pair already exists. Edit it instead.",
  "err.proposal_open": "A proposal for this entry is already waiting for approval.",
  "err.reference_required_when_checked":
    "Name the standard document you checked the entry against (reference) before marking it as checked.",
  "err.stale_proposal": "The entry changed after this proposal was made. Propose again.",
  "err.too_many_affected_decisions": "Too many decisions depend on this entry to flag them at once. Contact an administrator.",
  "err.grade_required": "Enter both grades.",
  "err.conditions_required": "A conditional equivalence needs its conditions.",
  "err.invalid_equivalence": "The equivalence must be identical, acceptable_with_conditions or not_equivalent.",
  "err.same_grade": "Both sides are the same grade.",
  "err.invalid_property": "The property key must be lowercase letters, digits and underscores.",
  "err.invalid_number": "Enter the limit as a number.",
  "err.bound_required": "Enter a minimum, a maximum or both.",
  "err.min_above_max": "The minimum is above the maximum.",
  "err.invalid_header": "The CSV header must be: a_standard,a_grade,b_standard,b_grade,equivalence,conditions,reference",
  "err.unterminated_quote": "The CSV file has an unterminated quote.",
  "err.too_many_rows": "The CSV file has too many rows (at most 2000).",
  "err.too_long": "A field is too long.",
  "err.control_characters": "A field contains control characters.",
};

export const gradesJa: Record<keyof typeof gradesEn, string> = {
  "err.entry_id_required": "変更する項目を選んでください。",
  "err.entry_not_effective": "この項目には変更できる承認済みの版がありません。",
  "err.four_eyes_required": "この提案の承認・却下は別の担当者が行う必要があります。",
  "err.grade_equivalence_changed":
    "この判断が依拠していた材質同等表の項目が変更または廃止されました。明細を再評価して判断し直してください。",
  "err.invalid_change": "変更の種類は create・edit・retire のいずれかです。",
  "err.invalid_note": "メモが長すぎるか、制御文字を含んでいます。",
  "err.limit_exists": "この材質と特性の承認済み限度がすでにあります。そちらを編集してください。",
  "err.no_equivalence_entry":
    "自社の材質同等表にこの組み合わせの承認済み項目がありません。先に項目を提案してください（別の担当者の承認が必要です）。",
  "err.no_open_substitution": "この明細には、この成績書の材質に対する未決の代替指摘がありません。",
  "err.not_proposed": "この項目はすでに判断済みです。",
  "err.pair_exists": "この材質の組み合わせの承認済み項目がすでにあります。そちらを編集してください。",
  "err.proposal_open": "この項目への提案がすでに承認待ちです。",
  "err.reference_required_when_checked": "確認済みにする前に、照合した規格文書（参照）を記入してください。",
  "err.stale_proposal": "提案後に項目が変更されました。もう一度提案してください。",
  "err.too_many_affected_decisions": "この項目に依拠する判断が多すぎて一度に通知できません。管理者に連絡してください。",
  "err.grade_required": "両方の材質を入力してください。",
  "err.conditions_required": "条件付き同等には条件の記入が必要です。",
  "err.invalid_equivalence": "同等性は identical・acceptable_with_conditions・not_equivalent のいずれかです。",
  "err.same_grade": "両側が同じ材質です。",
  "err.invalid_property": "特性キーは英小文字・数字・アンダースコアで入力してください。",
  "err.invalid_number": "限度は数値で入力してください。",
  "err.bound_required": "下限・上限の少なくとも一方を入力してください。",
  "err.min_above_max": "下限が上限を上回っています。",
  "err.invalid_header":
    "CSV の見出しは a_standard,a_grade,b_standard,b_grade,equivalence,conditions,reference である必要があります。",
  "err.unterminated_quote": "CSV に閉じられていない引用符があります。",
  "err.too_many_rows": "CSV の行数が多すぎます（最大 2000 行）。",
  "err.too_long": "入力が長すぎる項目があります。",
  "err.control_characters": "制御文字を含む項目があります。",
};
