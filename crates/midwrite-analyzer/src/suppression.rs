//! Source-local reporting directives. Effects remain in the IR and summaries.

use std::collections::BTreeSet;

use midwrite_core::diagnostic::{Diagnostic, DiagnosticCode};
use midwrite_core::location::LineIndex;
use oxc_ast::ast::{Comment, CommentKind};

use crate::ParseFailure;

#[derive(Clone, Debug, Default)]
pub(crate) struct Suppressions {
    targets: BTreeSet<(u32, DiagnosticCode)>,
}

impl Suppressions {
    pub(crate) fn parse(
        source: &str,
        comments: &[Comment],
        lines: &LineIndex<'_>,
    ) -> Result<Self, ParseFailure> {
        let mut targets = BTreeSet::new();
        for comment in comments {
            if comment.kind != CommentKind::Line {
                continue;
            }
            let span = comment.content_span();
            let text = source[span.start as usize..span.end as usize].trim();
            let Some(rest) = text.strip_prefix("midwrite-ignore") else {
                continue;
            };
            // A longer word is not this directive (including prose examples).
            if rest.starts_with(|c: char| !c.is_whitespace() && c != ':') {
                continue;
            }
            let (line, column) = lines.position(comment.span.start);
            let invalid = |detail: &str| ParseFailure {
                message: format!(
                    "invalid midwrite-ignore: {detail}; expected `// midwrite-ignore <RULE_ID>: <reason>`"
                ),
                line: Some(line),
                column: Some(column),
            };
            let (rule, reason) = rest
                .trim()
                .split_once(':')
                .ok_or_else(|| invalid("a rule ID and a non-empty reason are required"))?;
            if reason.trim().is_empty() {
                return Err(invalid("a non-empty reason is required"));
            }
            let code = rule
                .trim()
                .parse::<DiagnosticCode>()
                .map_err(|error| invalid(&error.to_string()))?;
            let before = &source[..comment.span.start as usize];
            let line_prefix = before.rsplit('\n').next().unwrap_or("");
            // Standalone comments target exactly the next line; trailing
            // comments target their own line. Never extend to a whole scope.
            let target = if line_prefix.trim().is_empty() {
                line + 1
            } else {
                line
            };
            targets.insert((target, code));
        }
        Ok(Self { targets })
    }

    pub(crate) fn suppresses(&self, diagnostic: &Diagnostic) -> bool {
        self.targets
            .contains(&(diagnostic.location.line, diagnostic.code))
    }
}
