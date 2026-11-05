//! Comparing card dumps with each other and with the EF table.

use serde_yaml::{Mapping, Value};

use crate::ef_table::{self, Access, Structure};
use crate::fcp::{COMPARED_OPERATIONS, get_str};

// The DEACTIVATE, ACTIVATE and INCREASE conditions are not in the EF
// table. The applet (FileSystem.java) derives them per EF from C.S0065-B
// §5.2: NEVER for the EFs fixed before personalization, PIN to ACTIVATE
// the IMSIs and TMSI, INCREASE on EF_COUNT only, and ADM otherwise.
const NEVER_ACTIVATE_EFS: [&str; 3] = ["RUIMID", "SPCS", "SF_EUIMID"];
const PIN_ACTIVATE_EFS: [&str; 3] = ["IMSI_M", "IMSI_T", "TMSI"];
const INCREASE_EFS: [&str; 1] = ["COUNT"];

const COMPARED_FIELDS: [&str; 5] = [
    "structure",
    "file_size",
    "record_length",
    "num_records",
    "sfi",
];

/// The comparable fields of one EF.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub fid: Value,
    pub fields: Vec<(&'static str, Value)>,
    pub access: Mapping,
}

impl View {
    fn field(&self, name: &str) -> &Value {
        self.fields
            .iter()
            .find(|(n, _)| *n == name)
            .map_or(&Value::Null, |(_, v)| v)
    }
}

fn num(n: u64) -> Value {
    Value::Number(n.into())
}

/// What the applet serves for every EF in the table.
pub fn expected_table() -> Vec<(&'static str, View)> {
    ef_table::entries()
        .iter()
        .map(|e| {
            let (deactivate, activate) = if NEVER_ACTIVATE_EFS.contains(&e.name.as_str()) {
                (Access::Never, Access::Never)
            } else if PIN_ACTIVATE_EFS.contains(&e.name.as_str()) {
                (Access::Adm, Access::Pin1)
            } else {
                (Access::Adm, Access::Adm)
            };
            let mut access = Mapping::new();
            access.insert("READ".into(), e.read.name().into());
            access.insert("UPDATE".into(), e.update.name().into());
            if e.structure == Structure::Cyclic {
                let inc = if INCREASE_EFS.contains(&e.name.as_str()) {
                    e.update
                } else {
                    Access::Never
                };
                access.insert("INCREASE".into(), inc.name().into());
            }
            access.insert("DEACTIVATE".into(), deactivate.name().into());
            access.insert("ACTIVATE".into(), activate.name().into());
            let transparent = e.structure == Structure::Transparent;
            let fields = vec![
                ("structure", Value::from(e.structure.name())),
                ("file_size", num(u64::from(e.size))),
                (
                    "record_length",
                    if transparent {
                        Value::Null
                    } else {
                        num(u64::from(e.record_length))
                    },
                ),
                (
                    "num_records",
                    if transparent {
                        Value::Null
                    } else {
                        num(u64::from(e.num_records()))
                    },
                ),
                ("sfi", num(u64::from(e.sfi))),
            ];
            (
                e.name.as_str(),
                View {
                    fid: e.fid.to_string().into(),
                    fields,
                    access,
                },
            )
        })
        .collect()
}

/// The comparable fields of one dump EF entry.
pub fn view_of_entry(entry: &Mapping) -> View {
    let get = |k: &str| entry.get(k).cloned().unwrap_or(Value::Null);
    let transparent = get_str(entry, "structure") == Some("transparent");
    let mut size = get("file_size");
    if size.is_null() {
        if let (Some(rl), Some(n)) = (get("record_length").as_u64(), get("num_records").as_u64()) {
            if rl > 0 && n > 0 {
                size = num(rl * n);
            }
        }
    }
    let sfi = match get("sfi") {
        Value::Null => num(0),
        v => v,
    };
    let fields = vec![
        ("structure", get("structure")),
        ("file_size", size),
        (
            "record_length",
            if transparent {
                Value::Null
            } else {
                get("record_length")
            },
        ),
        (
            "num_records",
            if transparent {
                Value::Null
            } else {
                get("num_records")
            },
        ),
        ("sfi", sfi),
    ];
    View {
        fid: get("fid"),
        fields,
        access: entry
            .get("access")
            .and_then(Value::as_mapping)
            .cloned()
            .unwrap_or_default(),
    }
}

fn present(entry: &Mapping) -> bool {
    matches!(
        get_str(entry, "select_sw").unwrap_or("9000"),
        "9000" | "6283"
    )
}

/// One differing field: (EF, field, left value, right value).
pub type Diff = (String, String, Value, Value);

pub fn diff_views(label: &str, a: &View, b: &View) -> Vec<Diff> {
    let mut out = Vec::new();
    for f in COMPARED_FIELDS {
        if a.field(f) != b.field(f) {
            out.push((
                label.to_owned(),
                f.to_owned(),
                a.field(f).clone(),
                b.field(f).clone(),
            ));
        }
    }
    for op in COMPARED_OPERATIONS {
        let (x, y) = (a.access.get(op), b.access.get(op));
        if (x.is_some() || y.is_some()) && x != y {
            let v = |o: Option<&Value>| o.cloned().unwrap_or(Value::Null);
            out.push((label.to_owned(), format!("access.{op}"), v(x), v(y)));
        }
    }
    out
}

fn present_efs(dump: &Mapping) -> Vec<(String, Mapping)> {
    let mut efs: Vec<(String, Mapping)> = dump
        .get("efs")
        .and_then(Value::as_mapping)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.as_str()?.to_owned(), v.as_mapping()?.clone())))
                .filter(|(_, v)| present(v))
                .collect()
        })
        .unwrap_or_default();
    efs.sort_by(|(ka, a), (kb, b)| {
        get_str(a, "fid")
            .unwrap_or(ka)
            .cmp(get_str(b, "fid").unwrap_or(kb))
    });
    efs
}

fn content_of(entry: &Mapping) -> Value {
    entry
        .get("raw")
        .or_else(|| entry.get("records"))
        .cloned()
        .unwrap_or(Value::Null)
}

pub struct DumpComparison {
    pub only_a: Vec<String>,
    pub only_b: Vec<String>,
    pub diffs: Vec<Diff>,
    pub content: Vec<(String, Value, Value)>,
    pub common: usize,
}

impl DumpComparison {
    pub fn differs(&self) -> bool {
        !self.diffs.is_empty() || !self.only_a.is_empty() || !self.only_b.is_empty()
    }
}

pub fn compare_dumps(a: &Mapping, b: &Mapping, content: bool) -> DumpComparison {
    let efs_a = present_efs(a);
    let efs_b = present_efs(b);
    let has = |efs: &[(String, Mapping)], k: &str| efs.iter().any(|(l, _)| l == k);
    let mut out = DumpComparison {
        only_a: efs_a
            .iter()
            .filter(|(k, _)| !has(&efs_b, k))
            .map(|(k, _)| k.clone())
            .collect(),
        only_b: efs_b
            .iter()
            .filter(|(k, _)| !has(&efs_a, k))
            .map(|(k, _)| k.clone())
            .collect(),
        diffs: Vec::new(),
        content: Vec::new(),
        common: 0,
    };
    for (label, ea) in &efs_a {
        let Some((_, eb)) = efs_b.iter().find(|(l, _)| l == label) else {
            continue;
        };
        out.common += 1;
        out.diffs
            .extend(diff_views(label, &view_of_entry(ea), &view_of_entry(eb)));
        if content {
            let (ca, cb) = (content_of(ea), content_of(eb));
            if ca != cb {
                out.content.push((label.clone(), ca, cb));
            }
        }
    }
    out
}

pub struct TableComparison {
    pub missing: Vec<String>,
    pub extra: Vec<String>,
    pub diffs: Vec<Diff>,
    pub common: usize,
}

impl TableComparison {
    pub fn differs(&self) -> bool {
        !self.diffs.is_empty() || !self.missing.is_empty()
    }
}

pub fn compare_to_table(dump: &Mapping) -> TableComparison {
    let table = expected_table();
    let efs = present_efs(dump);
    let in_table = |k: &str| table.iter().any(|(n, _)| *n == k);
    let mut out = TableComparison {
        missing: table
            .iter()
            .filter(|(n, _)| !efs.iter().any(|(k, _)| k == n))
            .map(|(n, _)| (*n).to_owned())
            .collect(),
        extra: efs
            .iter()
            .filter(|(k, _)| !in_table(k))
            .map(|(k, _)| k.clone())
            .collect(),
        diffs: Vec::new(),
        common: 0,
    };
    for (label, expected) in &table {
        let Some((_, entry)) = efs.iter().find(|(k, _)| k == label) else {
            continue;
        };
        out.common += 1;
        out.diffs
            .extend(diff_views(label, &view_of_entry(entry), expected));
    }
    out
}

fn value_text(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Sequence(items) => items.iter().map(value_text).collect::<Vec<_>>().join(" / "),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_owned(),
    }
}

fn short(v: &Value, limit: usize) -> String {
    let s = value_text(v);
    if s.chars().count() <= limit {
        s
    } else {
        format!("{}...", s.chars().take(limit - 3).collect::<String>())
    }
}

fn format_diffs(diffs: &[Diff], left: &str, right: &str) -> Vec<String> {
    if diffs.is_empty() {
        return Vec::new();
    }
    let w = diffs
        .iter()
        .map(|(l, ..)| l.len())
        .max()
        .unwrap_or(2)
        .max(2);
    let mut lines = vec![format!(
        "  {:w$}  {:18}  {:22}  {right}",
        "EF", "field", left
    )];
    for (label, field, a, b) in diffs {
        lines.push(format!(
            "  {label:w$}  {field:18}  {:22}  {}",
            short(a, 22),
            short(b, 22)
        ));
    }
    lines
}

pub fn format_dump_report(r: &DumpComparison, left: &str, right: &str) -> String {
    let mut lines = Vec::new();
    if !r.only_a.is_empty() {
        lines.push(format!(
            "EFs only on {left} ({}): {}",
            r.only_a.len(),
            r.only_a.join(" ")
        ));
    }
    if !r.only_b.is_empty() {
        lines.push(format!(
            "EFs only on {right} ({}): {}",
            r.only_b.len(),
            r.only_b.join(" ")
        ));
    }
    lines.push(format!(
        "EFs on both: {}, metadata differences: {}",
        r.common,
        r.diffs.len()
    ));
    lines.extend(format_diffs(&r.diffs, left, right));
    if !r.content.is_empty() {
        lines.push(format!("content differs on {} EF(s):", r.content.len()));
        for (label, a, b) in &r.content {
            lines.push(format!("  {label}"));
            lines.push(format!("    {left}: {}", short(a, 96)));
            lines.push(format!("    {right}: {}", short(b, 96)));
        }
    }
    lines.join("\n")
}

pub fn format_table_report(r: &TableComparison, name: &str) -> String {
    let mut lines = Vec::new();
    if !r.missing.is_empty() {
        lines.push(format!(
            "EFs in the table but not on {name} ({}): {}",
            r.missing.len(),
            r.missing.join(" ")
        ));
    }
    if !r.extra.is_empty() {
        lines.push(format!(
            "EFs on {name} that are not in the table ({}): {}",
            r.extra.len(),
            r.extra.join(" ")
        ));
    }
    lines.push(format!(
        "EFs in both: {}, differences: {}",
        r.common,
        r.diffs.len()
    ));
    lines.extend(format_diffs(&r.diffs, name, "table"));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(yaml: &str) -> Mapping {
        serde_yaml::from_str(yaml).unwrap()
    }

    const IMSI: &str = "{fid: 6F22, select_sw: '9000', structure: transparent, file_size: 10, sfi: 4, \
                        access: {READ: PIN1, UPDATE: ADM1, DEACTIVATE: ADM1, ACTIVATE: PIN1}}";

    #[test]
    fn expected_table_entries() {
        let t = expected_table();
        assert_eq!(t.len(), 104);
        let imsi = &t.iter().find(|(n, _)| *n == "IMSI_M").unwrap().1;
        assert_eq!(*imsi, view_of_entry(&entry(IMSI)));
        let count = &t.iter().find(|(n, _)| *n == "COUNT").unwrap().1;
        assert_eq!(count.access["INCREASE"], "PIN1");
        assert_eq!(*count.field("num_records"), num(10));
        for name in ["ICI", "OCI", "UpBAKPARA"] {
            assert_eq!(
                t.iter().find(|(n, _)| *n == name).unwrap().1.access["INCREASE"],
                "NEV"
            );
        }
        for name in ["RUIMID", "SPCS", "SF_EUIMID"] {
            let v = &t.iter().find(|(n, _)| *n == name).unwrap().1;
            assert_eq!(
                (&v.access["DEACTIVATE"], &v.access["ACTIVATE"]),
                (&"NEV".into(), &"NEV".into())
            );
        }
    }

    #[test]
    fn diffs() {
        let a = view_of_entry(&entry(IMSI));
        assert!(diff_views("IMSI_M", &a, &a.clone()).is_empty());
        let b = view_of_entry(&entry(
            &IMSI
                .replace("file_size: 10", "file_size: 12")
                .replace("UPDATE: ADM1", "UPDATE: PIN1"),
        ));
        let d = diff_views("IMSI_M", &a, &b);
        assert_eq!(d.len(), 2);
        assert_eq!(
            (d[0].1.as_str(), &d[0].2, &d[0].3),
            ("file_size", &num(10), &num(12))
        );
        assert_eq!(d[1].1, "access.UPDATE");
    }

    #[test]
    fn dump_and_table_reports() {
        let mut efs = Mapping::new();
        efs.insert("IMSI_M".into(), Value::Mapping(entry(IMSI)));
        efs.insert(
            "EF_6FAA".into(),
            Value::Mapping(entry(
                "{fid: 6FAA, select_sw: '9000', structure: transparent, file_size: 1}",
            )),
        );
        let mut dump = Mapping::new();
        dump.insert("efs".into(), Value::Mapping(efs));
        let t = compare_to_table(&dump);
        assert_eq!(t.common, 1);
        assert!(t.diffs.is_empty());
        assert_eq!(t.extra, ["EF_6FAA"]);
        assert_eq!(t.missing.len(), 103);
        let d = compare_dumps(&dump, &dump, true);
        assert!(!d.differs() && d.content.is_empty() && d.common == 2);
        assert!(format_table_report(&t, "ours").contains("EFs in both: 1, differences: 0"));
    }
}
