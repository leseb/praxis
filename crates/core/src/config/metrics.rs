// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 Praxis Contributors

//! Metrics and observability configuration.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// -----------------------------------------------------------------------------
// MetricsConfig
// -----------------------------------------------------------------------------

/// Optional Prometheus metric collection settings.
///
/// All metrics default to disabled. Operators opt in per metric family.
///
/// ```
/// use praxis_core::config::MetricsConfig;
///
/// let metrics = MetricsConfig::default();
/// assert!(!metrics.filter_duration);
///
/// let metrics: MetricsConfig = serde_yaml::from_str("filter_duration: true").unwrap();
/// assert!(metrics.filter_duration);
/// ```
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MetricsConfig {
    /// Record per-filter hook duration histograms (`praxis_filter_duration_seconds`).
    pub filter_duration: bool,

    /// Path templates that collapse dynamic segments in the `route` label.
    ///
    /// Each entry is a path with `{name}` placeholders, e.g.
    /// `/users/{id}/orders`. A request whose path matches a template is
    /// labeled with the template instead of the router's path-match
    /// pattern, keeping the label stable and bounded while staying more
    /// precise than a prefix match.
    pub route_templates: Vec<String>,
}

// -----------------------------------------------------------------------------
// RouteTemplates
// -----------------------------------------------------------------------------

/// One path segment of a compiled route template.
#[derive(Clone, Debug, PartialEq, Eq)]
enum TemplateSegment {
    /// Must equal the request's segment.
    Literal(Box<str>),

    /// Matches any single segment.
    Placeholder,
}

/// A compiled route template and the label it produces.
#[derive(Clone, Debug)]
struct CompiledTemplate {
    /// The template as written, used as the metric label value.
    label: Box<str>,

    /// Segments to match, in order.
    segments: Vec<TemplateSegment>,
}

/// Compiled route templates, indexed by segment count.
///
/// Requests are matched by walking their path segments, so lookup costs
/// O(path depth) against only the templates of the same length. No regular
/// expressions and no per-request allocation.
///
/// ```
/// use praxis_core::config::RouteTemplates;
///
/// let templates = RouteTemplates::compile(&["/users/{id}/orders".to_owned()]);
/// assert_eq!(
///     templates.match_path("/users/42/orders"),
///     Some("/users/{id}/orders")
/// );
/// assert_eq!(templates.match_path("/users/42"), None);
/// ```
#[derive(Clone, Debug, Default)]
pub struct RouteTemplates {
    /// Templates grouped by their segment count.
    by_len: HashMap<usize, Vec<CompiledTemplate>>,
}

impl RouteTemplates {
    /// Compile templates from their configured string form.
    ///
    /// Templates are matched in configuration order within a segment count,
    /// so an earlier entry wins when two templates could both match.
    #[must_use]
    pub fn compile(templates: &[String]) -> Self {
        let mut by_len: HashMap<usize, Vec<CompiledTemplate>> = HashMap::new();
        for template in templates {
            let segments: Vec<TemplateSegment> = split_path(template)
                .map(|segment| {
                    if segment.starts_with('{') && segment.ends_with('}') {
                        TemplateSegment::Placeholder
                    } else {
                        TemplateSegment::Literal(Box::from(segment))
                    }
                })
                .collect();
            by_len.entry(segments.len()).or_default().push(CompiledTemplate {
                label: Box::from(template.as_str()),
                segments,
            });
        }
        Self { by_len }
    }

    /// Whether any template is configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_len.is_empty()
    }

    /// Return the template matching `path`, if any.
    ///
    /// The query string is ignored. Matching is exact on segment count, so
    /// `/users/{id}` does not match `/users/42/orders`.
    #[must_use]
    pub fn match_path(&self, path: &str) -> Option<&str> {
        let path = path.split(['?', '#']).next().unwrap_or(path);
        let len = split_path(path).count();
        self.by_len.get(&len)?.iter().find_map(|template| {
            split_path(path)
                .zip(&template.segments)
                .all(|(segment, expected)| match expected {
                    TemplateSegment::Literal(literal) => segment == literal.as_ref(),
                    TemplateSegment::Placeholder => !segment.is_empty(),
                })
                .then(|| template.label.as_ref())
        })
    }
}

/// Split a path into its non-empty segments.
fn split_path(path: &str) -> impl Iterator<Item = &str> {
    path.split('/').filter(|segment| !segment.is_empty())
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
#[expect(clippy::allow_attributes, reason = "blanket test suppressions")]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, reason = "tests")]
mod tests {
    use super::*;

    #[test]
    fn defaults_filter_duration_off() {
        let metrics = MetricsConfig::default();
        assert!(!metrics.filter_duration, "filter_duration should default to false");
    }

    #[test]
    fn parse_empty_yields_defaults() {
        let metrics: MetricsConfig = serde_yaml::from_str("{}").unwrap();
        assert!(
            !metrics.filter_duration,
            "empty yaml should default filter_duration to false"
        );
    }

    #[test]
    fn route_templates_default_to_empty() {
        let metrics = MetricsConfig::default();
        assert!(
            metrics.route_templates.is_empty(),
            "route_templates should default empty"
        );
        assert!(
            RouteTemplates::compile(&metrics.route_templates).is_empty(),
            "no templates compiles to an empty matcher"
        );
    }

    #[test]
    fn route_templates_collapse_dynamic_segments() {
        let templates = RouteTemplates::compile(&["/users/{id}/orders".to_owned()]);
        for path in ["/users/1/orders", "/users/abc-def/orders", "/users/42/orders/"] {
            assert_eq!(
                templates.match_path(path),
                Some("/users/{id}/orders"),
                "{path} should collapse to the template"
            );
        }
    }

    #[test]
    fn route_templates_require_an_exact_segment_count() {
        let templates = RouteTemplates::compile(&["/users/{id}".to_owned()]);
        assert_eq!(
            templates.match_path("/users/42/orders"),
            None,
            "extra segments must not match"
        );
        assert_eq!(templates.match_path("/users"), None, "missing segments must not match");
    }

    #[test]
    fn route_templates_ignore_the_query_string() {
        let templates = RouteTemplates::compile(&["/search/{term}".to_owned()]);
        assert_eq!(
            templates.match_path("/search/shoes?page=2"),
            Some("/search/{term}"),
            "a query string must not defeat the match"
        );
    }

    #[test]
    fn route_templates_match_literals_exactly() {
        let templates = RouteTemplates::compile(&["/api/{version}/health".to_owned()]);
        assert_eq!(templates.match_path("/api/v1/health"), Some("/api/{version}/health"));
        assert_eq!(
            templates.match_path("/api/v1/status"),
            None,
            "literal mismatch must not match"
        );
    }

    #[test]
    fn route_templates_prefer_the_first_configured_match() {
        let templates = RouteTemplates::compile(&["/a/{x}".to_owned(), "/{y}/b".to_owned()]);
        assert_eq!(
            templates.match_path("/a/b"),
            Some("/a/{x}"),
            "configuration order decides when two templates overlap"
        );
    }

    #[test]
    fn parse_route_templates() {
        let metrics: MetricsConfig = serde_yaml::from_str("route_templates:\n  - \"/users/{id}\"").unwrap();
        assert_eq!(metrics.route_templates, vec!["/users/{id}".to_owned()]);
    }

    #[test]
    fn parse_explicit_filter_duration() {
        let metrics: MetricsConfig = serde_yaml::from_str("filter_duration: true").unwrap();
        assert!(metrics.filter_duration, "explicit filter_duration should be true");
    }
}
