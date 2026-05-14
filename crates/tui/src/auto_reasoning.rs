//! Adaptive reasoning-effort tier selection for `Auto` mode (#663).
//!
//! When the user sets `reasoning_effort = "auto"`, the engine calls
//! [`select`] before each turn-level request to pick the actual tier
//! based on the current message.

use crate::tui::app::ReasoningEffort;

/// Choose a concrete `ReasoningEffort` tier for the next API request.
///
/// Rules:
/// - Sub-agent contexts (`is_subagent == true`) → `Low`
/// - Last user message contains a high-effort keyword
///   (continuity/canon/memory conflicts, chapter revision, manuscript-wide
///   analysis, or explicit debugging/error recovery terms) → `Max`
/// - Last user message contains a low-effort keyword
///   (English: `search`, `lookup`; Chinese: 搜索 / 查找 / 查询;
///   Japanese: 検索) → `Low`
/// - Everything else → `High`
#[must_use]
pub fn select(is_subagent: bool, last_msg: &str) -> ReasoningEffort {
    if is_subagent {
        return ReasoningEffort::Low;
    }

    let lower = last_msg.to_ascii_lowercase();

    if HIGH_EFFORT_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
        return ReasoningEffort::Max;
    }

    if LOW_EFFORT_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
        return ReasoningEffort::Low;
    }

    ReasoningEffort::High
}

/// Keywords that bump `reasoning_effort` to `Max`. Latin terms are
/// lowercase because the caller lowercases the message; CJK has no
/// case so the literal form matches as-is. The first block is Novel
/// Studio-specific: these tasks tend to require deeper cross-asset
/// reasoning than ordinary chat or short wording help.
const HIGH_EFFORT_KEYWORDS: &[&str] = &[
    // Novel Studio task terms.
    "continuity",
    "canon",
    "memory conflict",
    "memory conflicts",
    "memory graph",
    "knowledge boundary",
    "foreshadowing",
    "reader promise",
    "character arc",
    "timeline",
    "manuscript",
    "chapter revision",
    "revise chapter",
    "audit chapter",
    "diagnose memory",
    "analyze manuscript",
    "/audit",
    "/revise",
    "/remember",
    "/analyze",
    // Explicit recovery/debugging terms remain useful for runtime maintenance
    // and user-reported failures.
    "debug",
    "error",
    // Simplified Chinese novel terms.
    "\u{8fde}\u{7eed}\u{6027}",         // 连续性
    "\u{8bb0}\u{5fc6}\u{51b2}\u{7a81}", // 记忆冲突
    "\u{8bb0}\u{5fc6}\u{56fe}",         // 记忆图
    "\u{77e5}\u{8bc6}\u{8fb9}\u{754c}", // 知识边界
    "\u{4f0f}\u{7b14}",                 // 伏笔
    "\u{8bfb}\u{8005}\u{627f}\u{8bfa}", // 读者承诺
    "\u{4eba}\u{7269}\u{5f27}\u{7ebf}", // 人物弧线
    "\u{65f6}\u{95f4}\u{7ebf}",         // 时间线
    "\u{5168}\u{4e66}\u{5206}\u{6790}", // 全书分析
    "\u{7ae0}\u{8282}\u{4fee}\u{8ba2}", // 章节修订
    "\u{5ba1}\u{8ba1}\u{7ae0}\u{8282}", // 审计章节
    "\u{8bca}\u{65ad}\u{8bb0}\u{5fc6}", // 诊断记忆
    // Simplified / Traditional Chinese runtime recovery terms.
    "\u{8c03}\u{8bd5}", // 调试
    "\u{9519}\u{8bef}", // 错误
    "\u{62a5}\u{9519}", // 报错
    "\u{51fa}\u{9519}", // 出错
    "\u{5d29}\u{6e83}", // 崩溃
    // Traditional Chinese novel terms.
    "\u{9023}\u{7e8c}\u{6027}",         // 連續性
    "\u{8a18}\u{61b6}\u{885d}\u{7a81}", // 記憶衝突
    "\u{8a18}\u{61b6}\u{5716}",         // 記憶圖
    "\u{77e5}\u{8b58}\u{908a}\u{754c}", // 知識邊界
    "\u{4f0f}\u{7b46}",                 // 伏筆
    "\u{8b80}\u{8005}\u{627f}\u{8afe}", // 讀者承諾
    "\u{4eba}\u{7269}\u{5f27}\u{7dda}", // 人物弧線
    "\u{6642}\u{9593}\u{7dda}",         // 時間線
    "\u{5168}\u{66f8}\u{5206}\u{6790}", // 全書分析
    "\u{7ae0}\u{7bc0}\u{4fee}\u{8a02}", // 章節修訂
    "\u{5be9}\u{8a08}\u{7ae0}\u{7bc0}", // 審計章節
    "\u{8a3a}\u{65b7}\u{8a18}\u{61b6}", // 診斷記憶
    "\u{8abf}\u{8a66}",                 // 調試
    "\u{932f}\u{8aa4}",                 // 錯誤
    // Japanese.
    "\u{30c7}\u{30d0}\u{30c3}\u{30b0}", // デバッグ
    "\u{30a8}\u{30e9}\u{30fc}",         // エラー
    "\u{30d0}\u{30b0}",                 // バグ
];

/// Keywords that drop `reasoning_effort` to `Low`. Same locale coverage
/// as [`HIGH_EFFORT_KEYWORDS`].
const LOW_EFFORT_KEYWORDS: &[&str] = &[
    "search",
    "lookup",
    "\u{641c}\u{7d22}", // 搜索
    "\u{67e5}\u{627e}", // 查找
    "\u{67e5}\u{8be2}", // 查询
    "\u{691c}\u{7d22}", // 検索
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subagent_returns_low() {
        assert_eq!(select(true, "anything"), ReasoningEffort::Low);
        assert_eq!(select(true, "debug this"), ReasoningEffort::Low);
        assert_eq!(select(true, "search query"), ReasoningEffort::Low);
    }

    #[test]
    fn debug_or_error_returns_max() {
        assert_eq!(select(false, "find a bug"), ReasoningEffort::High);
        assert_eq!(select(false, "debug crash"), ReasoningEffort::Max);
        assert_eq!(select(false, "Error: timeout"), ReasoningEffort::Max);
        assert_eq!(select(false, "fix this error"), ReasoningEffort::Max);
        assert_eq!(select(false, "DEBUG output"), ReasoningEffort::Max);
    }

    #[test]
    fn search_or_lookup_returns_low() {
        assert_eq!(select(false, "search for the file"), ReasoningEffort::Low);
        assert_eq!(select(false, "lookup docs"), ReasoningEffort::Low);
        assert_eq!(select(false, "SearchQuery"), ReasoningEffort::Low);
        assert_eq!(select(false, "lookup_user"), ReasoningEffort::Low);
    }

    #[test]
    fn default_returns_high() {
        assert_eq!(select(false, "hello"), ReasoningEffort::High);
        assert_eq!(select(false, "write a test"), ReasoningEffort::High);
        assert_eq!(select(false, "refactor this module"), ReasoningEffort::High);
        assert_eq!(select(false, ""), ReasoningEffort::High);
    }

    #[test]
    fn novel_studio_high_risk_tasks_return_max() {
        for msg in [
            "diagnose memory conflicts in chapter 12",
            "audit chapter 8 continuity",
            "revise chapter 12 final prose",
            "analyze manuscript knowledge boundary",
            "check foreshadowing and reader promise",
            "/remember 12",
            "/analyze 全书连续性",
        ] {
            assert_eq!(
                select(false, msg),
                ReasoningEffort::Max,
                "expected Max for `{msg}`",
            );
        }
    }

    #[test]
    fn chinese_debug_keywords_return_max() {
        // The original keyword set was English-only; Chinese-speaking
        // Auto-mode users paid for `High` even on real debugging tasks.
        for msg in [
            "\u{5e2e}\u{6211}\u{8c03}\u{8bd5}\u{4ee3}\u{7801}", // 帮我调试代码
            "\u{8fd9}\u{91cc}\u{6709}\u{4e2a}\u{9519}\u{8bef}", // 这里有个错误
            "\u{4ee3}\u{7801}\u{62a5}\u{9519}\u{4e86}",         // 代码报错了
            "\u{7a0b}\u{5e8f}\u{51fa}\u{9519}",                 // 程序出错
            "\u{7cfb}\u{7edf}\u{5d29}\u{6e83}",                 // 系统崩溃
            "\u{4ee3}\u{78bc}\u{8abf}\u{8a66}",                 // 代碼調試 (zh-Hant)
            "\u{6709}\u{500b}\u{932f}\u{8aa4}",                 // 有個錯誤 (zh-Hant)
        ] {
            assert_eq!(
                select(false, msg),
                ReasoningEffort::Max,
                "expected Max for `{msg}`",
            );
        }
    }

    #[test]
    fn japanese_debug_keywords_return_max() {
        for msg in [
            "\u{30b3}\u{30fc}\u{30c9}\u{3092}\u{30c7}\u{30d0}\u{30c3}\u{30b0}", // コードをデバッグ
            "\u{30a8}\u{30e9}\u{30fc}\u{304c}\u{51fa}\u{305f}",                 // エラーが出た
            "\u{30d0}\u{30b0}\u{3092}\u{4fee}\u{6b63}",                         // バグを修正
        ] {
            assert_eq!(
                select(false, msg),
                ReasoningEffort::Max,
                "expected Max for `{msg}`",
            );
        }
    }

    #[test]
    fn chinese_search_keywords_return_low() {
        for msg in [
            "\u{641c}\u{7d22}\u{4e00}\u{4e0b}\u{6587}\u{4ef6}", // 搜索一下文件
            "\u{5e2e}\u{6211}\u{67e5}\u{627e}\u{5b9a}\u{4e49}", // 帮我查找定义
            "\u{67e5}\u{8be2}\u{6587}\u{6863}",                 // 查询文档
        ] {
            assert_eq!(
                select(false, msg),
                ReasoningEffort::Low,
                "expected Low for `{msg}`",
            );
        }
    }

    #[test]
    fn japanese_search_keyword_returns_low() {
        // 検索 → "search"
        assert_eq!(
            select(
                false,
                "\u{30c9}\u{30ad}\u{30e5}\u{30e1}\u{30f3}\u{30c8}\u{691c}\u{7d22}"
            ),
            ReasoningEffort::Low,
        );
    }

    #[test]
    fn cjk_default_still_returns_high() {
        // No keyword hits — ordinary Chinese/Japanese prose stays on
        // the `High` default like English does.
        for msg in [
            "\u{5e2e}\u{6211}\u{5199}\u{4e2a}\u{6d4b}\u{8bd5}", // 帮我写个测试
            "\u{91cd}\u{6784}\u{8fd9}\u{4e2a}\u{6a21}\u{5757}", // 重构这个模块
            "\u{30c6}\u{30b9}\u{30c8}\u{3092}\u{66f8}\u{304f}", // テストを書く
        ] {
            assert_eq!(
                select(false, msg),
                ReasoningEffort::High,
                "expected High for `{msg}`",
            );
        }
    }
}
