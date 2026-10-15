//! Edit → Edit Colors → Recolor Artwork: list the selection's colours, reduce them to rows of
//! similar colours, and remap each row to a new colour by a recolour method. Colours are keyed by
//! their identity ([`ColorKey`]: model and exact values), so a CMYK colour stays CMYK and two
//! colours that only look alike stay apart.

use std::collections::HashMap;

use serde_json::{Value, json};
use vectorcraft_color::recolor::{ColorKey, Method, Preserve, Rng, RowMap, Sort, cluster, pair};
use vectorcraft_color::{Color, Paint, SwatchGroup};
use vectorcraft_doc::swatches::node_colors;
use vectorcraft_doc::{AppearanceItem, Document, NodeId, NodeKind};

use super::colorcmds::{Recolor, Scope};
use super::swatch::GroupEdit;
use super::*;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "recolor.colors",
            "Artwork Colors",
            [],
            None,
            "{} → {colors: [{key: colour identity (\"rgb 255 0 0\", \"cmyk 0 100 100 0\", \"gray 40\", \"lab 55 60 -40\"), hex, count, swatch?: the global swatch it is linked to}]} unique colours used by the selection (fills, strokes, gradient stops, text, mesh points and the tiles of pattern fills and strokes), most used first",
            has_selection,
            colors
        ),
        cmd!(
            "recolor.apply",
            "Recolor Artwork",
            ["Edit", "Edit Colors"],
            None,
            "{map: rows [{from: [colour keys (recolor.colors) or \"#rrggbb\" (every colour shown as that hex)], to: colour (hex, key, {c,m,y,k} or {l,a,b}) | null, exclude?: false (the row's colours are kept)}] | {\"key or #rrggbb\": colour, …} (one row per colour), method?: \"exact\" (default)|\"preserveTints\"|\"scaleTints\"|\"tintsShades\"|\"hueShift\" (how a row's colours take its new colour, relative to the row's darkest, average or most saturated colour), limitTo?: swatch library id or name; or \"document\" (the document's swatches): new colours snap to its nearest colour, group?: colour group to rewrite, groupColors?: [colour] (what `group` is rewritten with; default: the rows' new colours, in order, excluded rows left out), rename?: the group's new name, groupEdits?: [{group?: existing colour group (omitted: a new group), name?: its (new) name, colors?: [colour] (as swatch.editGroup), delete?: false}] (Recolor Artwork's Color Groups list: groups deleted, rewritten or added before `group` is rewritten; a new group named like `group` is the one `group` rewrites), includeImages?: true, includePatterns?: true} recolour the selection (gradients, text, meshes, image pixels and pattern tiles included; each new colour keeps the model of the one it replaces) and the groups, as one undo step → {changed, groups: {name given: name it got} for the new groups}",
            has_doc,
            apply
        ),
        cmd!(
            query "recolor.reduce",
            "Reduce Colors",
            [],
            None,
            "{colors?: n (rows; default: one per colour, with the tints of a global swatch in its row), method?: as recolor.apply; default scaleTints (picks each row's colour), preserve?: {white?: true, black?: true, grays?: false} (left out of the rows), limitTo?: as recolor.apply (new colours snap to it), sort?: \"none\" (default: most used first)|\"hueForward\"|\"hueBackward\"|\"darkToLight\"|\"lightToDark\" (the rows' order, by their colour; neutrals last by hue), assign?: [colour] (new colours for the rows, e.g. a colour group's: sorted the same way as the rows and paired with them, the darkest row taking the darkest colour; with more colours than rows the rows take colours spread over the sorted range; without a sort the k-th row takes the k-th colour; a row past the last colour keeps its own)} group the selection's colours into rows of similar colours (k-means in Lab, weighted by use) → {map: [{from: [keys, the row's colour first], to: key, source?: index in `assign` of the colour the row took}], preserved: [keys], method, sort}",
            has_selection,
            reduce
        ),
        cmd!(
            query "recolor.randomize",
            "Randomize Colors",
            [],
            None,
            "{map: rows as recolor.apply, order?: true (shuffle the new colours among the rows that aren't excluded), saturationBrightness?: false (random saturation and brightness, hue kept), seed?: 0} → {map: the rows with their new colours as keys}",
            always,
            randomize
        ),
    ]
}

/// A colour the art uses: how often, and the global swatch it is linked to.
struct Used {
    color: Color,
    count: usize,
    swatch: Option<String>,
}

/// The unique colours of the selection, most used first (ties by key): fills, strokes, gradient
/// stops, text, mesh points and the tiles of the patterns used as fills or strokes.
fn used_colors(d: &Document, ids: &[NodeId]) -> Vec<Used> {
    let mut index: HashMap<ColorKey, usize> = HashMap::new();
    let mut used: Vec<(ColorKey, Used)> = vec![];
    let mut count = |c: &Color, link: Option<&str>| {
        let k = ColorKey::of(c);
        let i = *index.entry(k).or_insert_with(|| {
            used.push((k, Used { color: *c, count: 0, swatch: None }));
            used.len() - 1
        });
        let u = &mut used[i].1;
        u.count += 1;
        if u.swatch.is_none() {
            u.swatch = link.filter(|l| d.swatch(l).is_some_and(|w| w.global && w.paint.color().is_some())).map(str::to_string);
        }
    };
    // The patterns used as fills or strokes: their tiles count once each.
    let mut patterns: Vec<&str> = vec![];
    for n in ids.iter().filter_map(|id| d.node(*id)) {
        node_colors(n, &mut count);
        n.walk(&mut |m| {
            let runs = match &m.kind {
                NodeKind::Text(t) => t.runs.as_slice(),
                _ => &[],
            };
            let items = m.appearance.items.iter().map(|it| match it {
                AppearanceItem::Fill(l) => &l.paint,
                AppearanceItem::Stroke(l) => &l.paint,
            });
            for p in items.chain(runs.iter().flat_map(|r| [&r.style.fill, &r.style.stroke])) {
                if let Paint::Pattern { pattern, .. } = p
                    && !patterns.contains(&pattern.as_str())
                {
                    patterns.push(pattern);
                }
            }
        });
    }
    for def in patterns.iter().filter_map(|p| d.pattern(p)) {
        def.art.iter().for_each(|n| node_colors(n, &mut count));
    }
    used.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(a.0.cmp(&b.0)));
    used.into_iter().map(|(_, u)| u).collect()
}

fn colors(s: &mut Session, _: &Value) -> Result<Value> {
    let st = s.doc()?;
    let v: Vec<Value> = used_colors(&st.doc, &st.selection.objects)
        .iter()
        .map(|u| {
            let mut v = json!({"key": key_str(&u.color), "hex": u.color.to_hex(), "count": u.count});
            if let Some(w) = &u.swatch {
                v["swatch"] = json!(w);
            }
            v
        })
        .collect();
    Ok(json!({ "colors": v }))
}

/// The `method` parameter (`default` without one).
fn method_param(p: &Value, cmd: &str, default: Method) -> Result<Method> {
    match str_param(p, "method") {
        None => Ok(default),
        Some(m) => Method::parse(m).ok_or_else(|| {
            let ids: Vec<&str> = Method::ALL.iter().map(|m| m.id()).collect();
            bad(cmd, format!("unknown method `{m}` ({})", ids.join("|")))
        }),
    }
}

/// The `sort` parameter (none without one).
fn sort_param(p: &Value, cmd: &str) -> Result<Sort> {
    match str_param(p, "sort") {
        None => Ok(Sort::None),
        Some(m) => Sort::parse(m).ok_or_else(|| {
            let ids: Vec<&str> = Sort::ALL.iter().map(|m| m.id()).collect();
            bad(cmd, format!("unknown sort `{m}` ({})", ids.join("|")))
        }),
    }
}

fn key_str(c: &Color) -> Value {
    json!(ColorKey::of(c).to_string())
}

fn reduce(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "recolor.reduce";
    let method = method_param(p, C, Method::ScaleTints)?;
    let palette = super::swatchlib::limit_param(s, p, C)?;
    let sort = sort_param(p, C)?;
    let assign: Option<Vec<Color>> = match p.get("assign") {
        None | Some(Value::Null) => None,
        Some(Value::Array(cs)) => {
            Some(cs.iter().map(|v| color_value(v).ok_or_else(|| bad(C, format!("bad colour {v} in assign")))).collect::<Result<_>>()?)
        }
        Some(v) => return Err(bad(C, format!("`assign` takes a list of colours, not {v}"))),
    };
    let def = Preserve::default();
    let flag = |k: &str, d: bool| p.get("preserve").map_or(d, |v| bool_or(v, k, d));
    let preserve = Preserve { white: flag("white", def.white), black: flag("black", def.black), grays: flag("grays", def.grays) };
    let st = s.doc()?;
    let (kept, used): (Vec<Used>, Vec<Used>) = used_colors(&st.doc, &st.selection.objects).into_iter().partition(|u| preserve.keeps(&u.color));
    // Units that stay together: the tints of a global swatch, else one colour each.
    let mut units: Vec<Vec<&Used>> = vec![];
    for u in &used {
        match units.iter_mut().find(|g| u.swatch.is_some() && g[0].swatch == u.swatch) {
            Some(g) => g.push(u),
            None => units.push(vec![u]),
        }
    }
    let unit_colors = |g: &[&Used]| g.iter().map(|u| u.color).collect::<Vec<_>>();
    let rows: Vec<Vec<&Used>> = match p.get("colors").and_then(Value::as_u64) {
        Some(n) if (n as usize) < units.len() => {
            let keys: Vec<Color> = units.iter().map(|g| method.key_color(&unit_colors(g)).unwrap_or_default()).collect();
            let weights: Vec<f32> = units.iter().map(|g| g.iter().map(|u| u.count as f32).sum()).collect();
            cluster(&keys, &weights, n as usize).into_iter().map(|ix| ix.into_iter().flat_map(|i| units[i].iter().copied()).collect()).collect()
        }
        _ => units,
    };
    // Each row's colours, the row's colour first, in `sort` order.
    let mut rows: Vec<(Vec<Color>, Color)> = rows
        .iter()
        .map(|row| {
            let mut from = unit_colors(row);
            let key = method.key_color(&from).unwrap_or_default();
            if let Some(i) = from.iter().position(|c| *c == key) {
                from[..=i].rotate_right(1);
            }
            (from, key)
        })
        .collect();
    let keys: Vec<Color> = rows.iter().map(|r| r.1).collect();
    let order = sort.order(&keys);
    rows = order.iter().filter_map(|&i| rows.get(i).cloned()).collect();
    let keys: Vec<Color> = rows.iter().map(|r| r.1).collect();
    let sources = assign.as_ref().map(|a| pair(&keys, a, sort));
    let map: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(i, (from, key))| {
            let source = sources.as_ref().and_then(|s| s.get(i).copied().flatten());
            let new = source.and_then(|j| assign.as_ref().and_then(|a| a.get(j).copied())).unwrap_or(*key);
            let to = palette.as_ref().map_or(new, |pl| pl.nearest(new));
            let mut row = json!({"from": from.iter().map(key_str).collect::<Vec<_>>(), "to": key_str(&to)});
            if let Some(j) = source {
                row["source"] = json!(j);
            }
            row
        })
        .collect();
    Ok(json!({"map": map, "preserved": kept.iter().map(|u| key_str(&u.color)).collect::<Vec<_>>(), "method": method.id(), "sort": sort.id()}))
}

/// A current colour of a row: an exact colour, or every colour shown as a hex.
enum Match {
    Key(ColorKey),
    Hex(String),
}

impl Match {
    /// A colour key, a hex (`#rrggbb`) or a colour value.
    fn of(v: &Value) -> Option<Self> {
        if let Some(s) = v.as_str() {
            if let Some(k) = ColorKey::parse(s) {
                return Some(Match::Key(k));
            }
            return Color::from_hex(s).map(|c| Match::Hex(c.to_hex()));
        }
        color_value(v).map(|c| Match::Key(ColorKey::of(&c)))
    }
    fn color(&self) -> Color {
        match self {
            Match::Key(k) => k.color(),
            Match::Hex(h) => Color::from_hex(h).unwrap_or_default(),
        }
    }
    fn to_json(&self) -> Value {
        match self {
            Match::Key(k) => json!(k.to_string()),
            Match::Hex(h) => json!(h),
        }
    }
}

/// A row of the map: its current colours and its new colour; an excluded row keeps its colours.
struct Row {
    from: Vec<Match>,
    to: Option<Color>,
    exclude: bool,
}

impl Row {
    /// The new colour, unless the row is excluded.
    fn new_color(&self) -> Option<Color> {
        self.to.filter(|_| !self.exclude)
    }
}

/// The rows of a `map` parameter, in order: an array of `{from, to, exclude?}` or an object of
/// `current: new` pairs.
fn parse_rows(p: &Value, cmd: &str) -> Result<Vec<Row>> {
    let new = |v: Option<&Value>| -> Result<Option<Color>> {
        match v {
            None | Some(Value::Null) => Ok(None),
            Some(v) => color_value(v).map(Some).ok_or_else(|| bad(cmd, format!("bad new colour {v}"))),
        }
    };
    let cur = |v: &Value| Match::of(v).ok_or_else(|| bad(cmd, format!("bad current colour {v}")));
    match p.get("map") {
        Some(Value::Object(m)) => m.iter().map(|(k, v)| Ok(Row { from: vec![cur(&json!(k))?], to: new(Some(v))?, exclude: false })).collect(),
        Some(Value::Array(rows)) => rows
            .iter()
            .map(|r| {
                let from = match r.get("from") {
                    Some(Value::Array(a)) => a.iter().map(cur).collect::<Result<Vec<_>>>()?,
                    Some(v) => vec![cur(v)?],
                    None => return Err(bad(cmd, "each row needs `from`")),
                };
                Ok(Row { from, to: new(r.get("to"))?, exclude: bool_or(r, "exclude", false) })
            })
            .collect(),
        _ => Err(bad(cmd, "missing `map` (rows [{from, to}] or {current: new})")),
    }
}

/// A parsed map ready to recolour: the row of each current colour.
struct ColorMap {
    rows: Vec<RowMap>,
    keys: HashMap<ColorKey, usize>,
    hexes: HashMap<String, usize>,
}

impl ColorMap {
    fn new(rows: &[Row], method: Method) -> Self {
        let mut out = Self { rows: vec![], keys: HashMap::new(), hexes: HashMap::new() };
        for r in rows.iter().filter(|r| !r.from.is_empty()) {
            let Some(to) = r.new_color() else { continue };
            let i = out.rows.len();
            let colors: Vec<Color> = r.from.iter().map(Match::color).collect();
            out.rows.push(RowMap::new(method, &colors, to));
            for m in &r.from {
                match m {
                    Match::Key(k) => out.keys.entry(*k).or_insert(i),
                    Match::Hex(h) => out.hexes.entry(h.clone()).or_insert(i),
                };
            }
        }
        out
    }

    fn map(&self, c: Color) -> Color {
        let row = self.keys.get(&ColorKey::of(&c)).or_else(|| if self.hexes.is_empty() { None } else { self.hexes.get(&c.to_hex()) });
        row.map_or(c, |i| self.rows[*i].map(c))
    }
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "recolor.apply";
    let method = method_param(p, C, Method::Exact)?;
    let palette = super::swatchlib::limit_param(s, p, C)?;
    let mut rows = parse_rows(p, C)?;
    if let Some(pl) = &palette {
        for r in &mut rows {
            r.to = r.to.map(|c| pl.nearest(c));
        }
    }
    let ids = s.doc()?.selection.objects.clone();
    let group = str_param(p, "group");
    let edits = group_edits(p, C)?;
    if ids.is_empty() && group.is_none() && edits.is_empty() {
        return Err(bad(C, "select artwork to recolour, or give a colour `group`"));
    }
    let map = ColorMap::new(&rows, method);
    let f = |c: Color| map.map(c);
    let scope = Scope { fill: true, stroke: true, ..Scope::of(p) };
    let label = if ids.is_empty() { "Edit Color Group" } else { "Recolor Artwork" };
    let new: Vec<Color> = match p.get("groupColors").and_then(Value::as_array) {
        Some(cs) => cs
            .iter()
            .map(|v| {
                color_value(v).map(|c| palette.as_ref().map_or(c, |pl| pl.nearest(c))).ok_or_else(|| bad(C, format!("bad colour {v} in groupColors")))
            })
            .collect::<Result<_>>()?,
        None => rows.iter().filter_map(Row::new_color).collect(),
    };
    let rename = str_param(p, "rename");
    let (changed, done, names) = s.edit(label, |d, _| {
        let changed = Recolor::new(scope, &f).run(d, &ids);
        let (mut done, names) = apply_group_edits(d, &edits, C)?;
        // `group` may be one of the groups just added (under the name it got).
        let group = group.map(|g| names.get(g).and_then(Value::as_str).unwrap_or(g));
        if let Some(g) = group {
            done.push(super::swatch::rewrite_group(d, g, &new, rename, C)?);
        }
        Ok((changed, done, names))
    })?;
    done.iter().for_each(|e| e.defaults(s));
    Ok(json!({ "changed": changed, "groups": names }))
}

/// One entry of `groupEdits`: an existing colour group to delete or rewrite, or a new one.
struct GroupEditParam {
    group: Option<String>,
    name: Option<String>,
    colors: Option<Vec<Color>>,
    delete: bool,
}

/// The `groupEdits` parameter.
fn group_edits(p: &Value, cmd: &str) -> Result<Vec<GroupEditParam>> {
    let Some(v) = p.get("groupEdits").filter(|v| !v.is_null()) else { return Ok(vec![]) };
    let list = v.as_array().ok_or_else(|| bad(cmd, "`groupEdits` takes a list of {group?, name?, colors?, delete?}"))?;
    list.iter()
        .map(|e| {
            let text = |k: &str| e.get(k).and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
            let colors = match e.get("colors") {
                None | Some(Value::Null) => None,
                Some(Value::Array(cs)) => Some(
                    cs.iter().map(|v| color_value(v).ok_or_else(|| bad(cmd, format!("bad colour {v} in groupEdits")))).collect::<Result<Vec<_>>>()?,
                ),
                Some(v) => return Err(bad(cmd, format!("groupEdits: `colors` takes a list of colours, not {v}"))),
            };
            let g = GroupEditParam { group: text("group"), name: text("name"), colors, delete: bool_or(e, "delete", false) };
            if g.group.is_none() && g.delete {
                return Err(bad(cmd, "groupEdits: `delete` needs the `group` to delete"));
            }
            Ok(g)
        })
        .collect()
}

/// Apply `groupEdits` to `d`: deletions, then rewrites and renames of existing groups, then new
/// groups (named after the given name, made unique). Returns the edits (for the default paints)
/// and the names the new groups got, by the name they were given.
fn apply_group_edits(d: &mut Document, edits: &[GroupEditParam], cmd: &str) -> Result<(Vec<GroupEdit>, serde_json::Map<String, Value>)> {
    let mut done = vec![];
    let mut names = serde_json::Map::new();
    for e in edits.iter().filter(|e| e.delete) {
        let Some(g) = e.group.as_deref() else { continue };
        // Rewriting with no colours drops its swatches and unlinks the art linked to them.
        done.push(super::swatch::rewrite_group(d, g, &[], None, cmd)?);
        d.swatch_groups.retain(|x| x.name != g);
    }
    for e in edits.iter().filter(|e| !e.delete) {
        let Some(g) = e.group.as_deref() else { continue };
        let colors = match &e.colors {
            Some(cs) => cs.clone(),
            None => d
                .swatch_groups
                .iter()
                .find(|x| x.name == g)
                .ok_or_else(|| bad(cmd, format!("no colour group `{g}`")))?
                .swatches
                .iter()
                .filter_map(|w| w.paint.color())
                .collect(),
        };
        done.push(super::swatch::rewrite_group(d, g, &colors, e.name.as_deref(), cmd)?);
    }
    for e in edits.iter().filter(|e| e.group.is_none()) {
        let given = e.name.clone().unwrap_or_else(|| "Color Group".into());
        let name = d.free_swatch_name(&given);
        d.swatch_groups.push(SwatchGroup { name: name.clone(), swatches: vec![] });
        done.push(super::swatch::rewrite_group(d, &name, e.colors.as_deref().unwrap_or_default(), None, cmd)?);
        names.insert(given, json!(name));
    }
    Ok((done, names))
}

fn randomize(_: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "recolor.randomize";
    let rows = parse_rows(p, C)?;
    let mut rng = Rng::new(p.get("seed").and_then(Value::as_u64).unwrap_or(0));
    let mut news: Vec<Color> = rows.iter().filter_map(Row::new_color).collect();
    if bool_or(p, "order", true) {
        rng.shuffle(&mut news);
    }
    if bool_or(p, "saturationBrightness", false) {
        news = news.into_iter().map(|c| rng.saturation_brightness(c)).collect();
    }
    let mut news = news.into_iter();
    let map: Vec<Value> = rows
        .iter()
        .map(|r| {
            let to = if r.new_color().is_some() { news.next() } else { r.to };
            let mut row = json!({"from": r.from.iter().map(Match::to_json).collect::<Vec<_>>(), "to": to.as_ref().map(key_str)});
            if r.exclude {
                row["exclude"] = json!(true);
            }
            row
        })
        .collect();
    Ok(json!({ "map": map }))
}
