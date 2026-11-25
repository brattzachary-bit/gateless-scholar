//! Charts the model specifies as a fenced ```` ```chart {json} ``` ```` block. Parsed leniently
//! into a [`ChartSpec`], then validated: it must cite at least one supplied source, and every
//! number it plots must appear in the cited sources' text (formatting differences such as
//! "1,200" vs 1200 or "12%" vs 12 are allowed). Flow diagrams need no numbers but still need a
//! citation. Charts that fail are dropped and counted.

use std::collections::HashSet;

use serde_json::Value;

use crate::model::{ChartKind, ChartSeries, ChartSpec, Chunk, VennSet};

use super::json::{extract_json, int_list, str_field};

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let t: String = s.chars().filter(|c| c.is_ascii_digit() || matches!(c, '.' | '-')).collect();
            t.parse().ok()
        }
        _ => None,
    }
}

fn label(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Object(_) => str_field(v, "label").or_else(|| str_field(v, "name")).or_else(|| str_field(v, "text")),
        _ => None,
    }
}

/// Parse the JSON body of a chart block. Accepts points as `[x, y]` or `{"x":…, "y":…}`
/// (also `label`/`value`), nodes as strings or `{label}`, edges as `[from, to, label]` or
/// `{from, to, label}`.
pub fn parse_chart(body: &str) -> Option<ChartSpec> {
    let v = extract_json(body)?;
    let kind = match v.get("kind").or(v.get("type")).and_then(|k| k.as_str())?.trim().to_lowercase().as_str() {
        "bar" | "column" => ChartKind::Bar,
        "line" => ChartKind::Line,
        "venn" => ChartKind::Venn,
        "stock" | "candlestick" => ChartKind::Stock,
        "flow" | "flowchart" | "diagram" => ChartKind::Flow,
        _ => return None,
    };
    let point = |p: &Value| -> Option<(String, f64)> {
        match p {
            Value::Array(a) if a.len() >= 2 => Some((label(&a[0])?, num(&a[1])?)),
            Value::Object(_) => {
                let x = p.get("x").or(p.get("label")).or(p.get("name")).and_then(label)?;
                let y = p.get("y").or(p.get("value")).and_then(num)?;
                Some((x, y))
            }
            _ => None,
        }
    };
    let series: Vec<ChartSeries> = v
        .get("series")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    let points: Vec<(String, f64)> = s.get("points").or(s.get("data")).and_then(|p| p.as_array())?.iter().filter_map(point).collect();
                    Some(ChartSeries { name: str_field(s, "name").unwrap_or_default(), points })
                })
                .filter(|s| !s.points.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let sets: Vec<VennSet> = v
        .get("sets")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    let overlaps = s
                        .get("overlaps")
                        .and_then(|o| o.as_array())
                        .map(|o| {
                            o.iter()
                                .filter_map(|x| match x {
                                    Value::Array(a) if a.len() >= 2 => Some((num(&a[0])? as usize, num(&a[1])?)),
                                    Value::Object(_) => Some((x.get("index").or(x.get("with")).and_then(num)? as usize, x.get("size").or(x.get("value")).and_then(num)?)),
                                    _ => None,
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(VennSet { label: label(s)?, size: s.get("size").or(s.get("value")).and_then(num)?, overlaps })
                })
                .collect()
        })
        .unwrap_or_default();
    let nodes: Vec<String> = v.get("nodes").and_then(|n| n.as_array()).map(|a| a.iter().filter_map(label).collect()).unwrap_or_default();
    let edges: Vec<(usize, usize, String)> = v
        .get("edges")
        .and_then(|e| e.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|e| match e {
                    Value::Array(x) if x.len() >= 2 => Some((num(&x[0])? as usize, num(&x[1])? as usize, x.get(2).and_then(label).unwrap_or_default())),
                    Value::Object(_) => Some((e.get("from").and_then(num)? as usize, e.get("to").and_then(num)? as usize, e.get("label").and_then(label).unwrap_or_default())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let citations: Vec<u32> = int_list(&v, "citations").into_iter().chain(int_list(&v, "sources")).filter(|&n| n > 0).map(|n| n as u32).collect();
    Some(ChartSpec {
        kind,
        title: str_field(&v, "title").unwrap_or_default(),
        x_label: str_field(&v, "x_label"),
        y_label: str_field(&v, "y_label"),
        series,
        sets,
        nodes,
        edges,
        citations,
    })
}

/// All numbers written in a text, with "million"/"billion"/"thousand" scaled variants.
fn numbers_in_text(text: &str) -> Vec<f64> {
    let re = regex::Regex::new(r"(?i)(\d{1,3}(?:,\d{3})+|\d+)(\.\d+)?(\s*(thousand|million|billion|trillion))?").expect("regex");
    let mut out = Vec::new();
    for c in re.captures_iter(text) {
        let base: String = format!("{}{}", c[1].replace(',', ""), c.get(2).map(|m| m.as_str()).unwrap_or(""));
        let Ok(n) = base.parse::<f64>() else { continue };
        out.push(n);
        if let Some(scale) = c.get(4) {
            let m = match scale.as_str().to_lowercase().as_str() {
                "thousand" => 1e3,
                "million" => 1e6,
                "billion" => 1e9,
                _ => 1e12,
            };
            out.push(n * m);
        }
    }
    out
}

fn appears(value: f64, known: &[f64]) -> bool {
    let v = value.abs();
    known.iter().any(|&n| (n - v).abs() <= 1e-9 * v.max(1.0))
}

/// Validate a chart against the supplied sources. On success the chart's citations are
/// reduced to valid ones. Returns the reason on failure.
pub fn validate_chart(spec: &mut ChartSpec, valid: &HashSet<u32>, chunks: &[Chunk]) -> Result<(), String> {
    spec.citations.retain(|c| valid.contains(c));
    spec.citations.dedup();
    if spec.citations.is_empty() {
        return Err(format!("chart \"{}\" cites no supplied source", spec.title));
    }
    let known: Vec<f64> = chunks.iter().filter(|c| spec.citations.contains(&c.source_id)).flat_map(|c| numbers_in_text(&c.text)).collect();
    let check = |values: Vec<f64>| -> Result<(), String> {
        if values.is_empty() {
            return Err("chart has no data".into());
        }
        for v in values {
            if !appears(v, &known) {
                return Err(format!("value {v} does not appear in the cited sources"));
            }
        }
        Ok(())
    };
    match spec.kind {
        ChartKind::Bar | ChartKind::Line | ChartKind::Stock => check(spec.series.iter().flat_map(|s| s.points.iter().map(|p| p.1)).collect()),
        ChartKind::Venn => {
            if !(2..=3).contains(&spec.sets.len()) {
                return Err("a Venn diagram needs 2–3 sets".into());
            }
            let n = spec.sets.len();
            if spec.sets.iter().any(|s| s.overlaps.iter().any(|o| o.0 >= n)) {
                return Err("Venn overlap refers to a missing set".into());
            }
            check(spec.sets.iter().flat_map(|s| std::iter::once(s.size).chain(s.overlaps.iter().map(|o| o.1))).collect())
        }
        ChartKind::Flow => {
            if spec.nodes.len() < 2 {
                return Err("a flow diagram needs at least two steps".into());
            }
            let n = spec.nodes.len();
            if spec.edges.is_empty() {
                // Default: a simple chain.
                spec.edges = (1..n).map(|i| (i - 1, i, String::new())).collect();
            }
            if spec.edges.iter().any(|e| e.0 >= n || e.1 >= n) {
                return Err("flow edge refers to a missing step".into());
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks() -> Vec<Chunk> {
        vec![
            Chunk { source_id: 1, text: "Sales reached 1,200 units in 2022 and 1500 units in 2023, a 25% rise.".into(), section: None, score: 1.0 },
            Chunk { source_id: 2, text: "Around 3.4 million people were affected.".into(), section: None, score: 1.0 },
        ]
    }

    #[test]
    fn parses_lenient_chart() {
        let c = parse_chart(r#"{"kind":"Bar","title":"Sales","series":[{"name":"Units","points":[["2022",1200],{"x":"2023","y":"1,500"}]}],"citations":["[1]"]}"#).unwrap();
        assert_eq!(c.kind, ChartKind::Bar);
        assert_eq!(c.series[0].points, vec![("2022".to_string(), 1200.0), ("2023".to_string(), 1500.0)]);
        assert_eq!(c.citations, vec![1]);
        assert!(parse_chart("{\"kind\":\"pie\"}").is_none());
    }

    #[test]
    fn chart_numbers_must_be_in_cited_sources() {
        let valid: HashSet<u32> = [1, 2].into();
        let mut ok = parse_chart(r#"{"kind":"bar","title":"Sales","series":[{"name":"Units","points":[["2022",1200],["2023",1500]]}],"citations":[1]}"#).unwrap();
        assert!(validate_chart(&mut ok, &valid, &chunks()).is_ok());
        let mut pct = parse_chart(r#"{"kind":"bar","title":"Rise","series":[{"name":"%","points":[["rise",25]]}],"citations":[1]}"#).unwrap();
        assert!(validate_chart(&mut pct, &valid, &chunks()).is_ok());
        let mut scaled = parse_chart(r#"{"kind":"bar","title":"People","series":[{"name":"n","points":[["affected",3400000]]}],"citations":[2]}"#).unwrap();
        assert!(validate_chart(&mut scaled, &valid, &chunks()).is_ok());
        let mut invented = parse_chart(r#"{"kind":"line","title":"Sales","series":[{"name":"Units","points":[["2022",1200],["2024",1900]]}],"citations":[1]}"#).unwrap();
        assert!(validate_chart(&mut invented, &valid, &chunks()).is_err());
        let mut wrong_source = parse_chart(r#"{"kind":"bar","title":"Sales","series":[{"name":"Units","points":[["2022",1200]]}],"citations":[2]}"#).unwrap();
        assert!(validate_chart(&mut wrong_source, &valid, &chunks()).is_err());
        let mut uncited = parse_chart(r#"{"kind":"bar","title":"Sales","series":[{"name":"Units","points":[["2022",1200]]}],"citations":[7]}"#).unwrap();
        assert!(validate_chart(&mut uncited, &valid, &chunks()).is_err());
    }

    #[test]
    fn flow_needs_citation_not_numbers() {
        let valid: HashSet<u32> = [1].into();
        let mut f = parse_chart(r#"{"kind":"flow","title":"Pipeline","nodes":["Collect","Clean","Model"],"citations":[1]}"#).unwrap();
        assert!(validate_chart(&mut f, &valid, &chunks()).is_ok());
        assert_eq!(f.edges.len(), 2);
        let mut nf = parse_chart(r#"{"kind":"flow","title":"Pipeline","nodes":["Collect","Clean"]}"#).unwrap();
        assert!(validate_chart(&mut nf, &valid, &chunks()).is_err());
    }

    #[test]
    fn venn_values_checked() {
        let valid: HashSet<u32> = [1].into();
        let mut v = parse_chart(r#"{"kind":"venn","title":"Units","sets":[{"label":"A","size":1200,"overlaps":[[1,25]]},{"label":"B","size":1500}],"citations":[1]}"#).unwrap();
        assert!(validate_chart(&mut v, &valid, &chunks()).is_ok());
    }
}
