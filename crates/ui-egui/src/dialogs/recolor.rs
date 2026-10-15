//! Edit → Edit Colors → Recolor Artwork: the selection's colours in rows of current → new colours,
//! previewed live (engine: `recolor.reduce`, `recolor.randomize`, `recolor.apply`).
//!
//! Assign tab: Colors (Auto, or a count: similar colours merge into rows), Preserve White, Black and
//! Grays, the rows (click to select, Shift or Cmd/Ctrl-click to add; a row's arrow excludes or
//! includes it; the hex field edits its new colour), Merge and Separate, Randomize order or
//! saturation and brightness, Reset, the Method and Limit to Library. Edit tab: a hue and
//! saturation wheel with a marker per new colour (drag one; with linked handles they all turn and
//! scale together), a brightness slider and the harmony rules. The preset menu sets up a 1, 2 or 3
//! colour job or a library. Opened on a colour group (Edit or Apply Color Group), OK also rewrites
//! the group with the new colours (renamed by `groupName`); without art only the group changes.
//! Opened on colours alone (the Color Guide's Edit or Apply Colors without art), OK saves the new
//! colours as a colour group named `groupName`.
//!
//! Color Groups (beside the tabs): the document's colour groups. Clicking one makes it the active
//! group: the art is reduced to as many rows as it has colours, which become their new colours,
//! paired with the rows in Sort order (sorted alike, so the darkest art colour takes the group's
//! darkest colour). Below the list: Get Colors from Selected Art (back to the art's own colours,
//! no group), New Color Group (from the new colours), Save Changes to Color Group (the active
//! group takes the new colours, so they stay when another group is picked) and Delete Color Group.
//! The list changes are kept in the dialog and made on OK, in the same undo step as the recolour.
//!
//! Fields (`ui.dialog.set`): `rows` ([{from: [keys], to: key, exclude?}]), `colors` (null: Auto, or a
//! count), `method`, `preserveWhite`, `preserveBlack`, `preserveGrays`, `limitTo` (library id,
//! "document" for the document's swatches, "" for none), `group`, `groupName`, `tab` (`assign` or
//! `edit`), `rule` (harmony id), `linked`, `preview`, `sort` (`none`, `hueForward`, `hueBackward`,
//! `darkToLight`, `lightToDark`). A row may carry `source`: the index of the active group's colour
//! it took. Changing `colors`, a preserve flag, `limitTo` or `sort`, or picking a group, reduces the
//! rows again.

use egui::{Rect, Sense, Ui, pos2, vec2};
use serde_json::{Value, json};
use vectorcraft_color::harmony::{Harmony, move_on_wheel};
use vectorcraft_color::recolor::{ColorKey, Method, Sort};
use vectorcraft_color::{Color, keep_model};
use vectorcraft_engine::cmd::color_value;
use vectorcraft_engine::cmd::swatchlib;

use super::{DialogSpec, form};
use crate::panels::c32;
use crate::panels::swatches::{DOCUMENT_SWATCHES, colour_libraries, limit_key};
use crate::state::Dialog;
use crate::theme::{self, Tokens};
use crate::{VectorcraftApp, widgets};

/// The dialog kind of Recolor Artwork.
pub const KIND: &str = "recolor";

pub(super) const SPEC: DialogSpec = DialogSpec {
    heading: |_| tl!("Recolor Artwork").into(),
    body,
    confirm,
    preview: true,
    min_width: LEFT_W + GROUPS_W + 24.0,
    max_width: Some(LEFT_W + GROUPS_W + 40.0),
    ..DialogSpec::FORM
};

const CMD: &str = "recolor.apply";
/// Width of the current colours of a row.
const CURRENT_W: f32 = 250.0;
/// Diameter of the Edit tab's colour wheel.
const WHEEL: f32 = 220.0;
/// Height of the rows table, and of either tab (so switching tabs doesn't resize the dialog).
const TABLE_H: f32 = 220.0;
const TAB_H: f32 = 400.0;
/// Width of the tabs' column and of the Color Groups column beside it.
const LEFT_W: f32 = 530.0;
const GROUPS_W: f32 = 210.0;
/// Height of a colour group in the Color Groups list, and of its colour chips.
const GROUP_ROW_H: f32 = 36.0;
const GROUP_CHIP: f32 = 13.0;
const PRESETS: [&str; 5] = ["Custom", "1 Color Job", "2 Color Job", "3 Color Job", "Color Library"];

/// Open Recolor Artwork (`ui.recolorDialog {colors?, library?, group?}`) on the selected art and,
/// with `group`, on that colour group: art is reduced to as many rows as the group has colours,
/// which become their new colours; without art the rows are the group's colours. `colors` is a
/// count (an n-colour job) or the new colours to assign (without art or a group they are the
/// rows, which OK saves as a new colour group); `library` limits to a library or "document" (""
/// picks the first library).
pub fn open(app: &mut VectorcraftApp, p: &Value) -> Result<Value, String> {
    let st = app.session.active().ok_or("no document open")?;
    let art = !st.selection.is_empty();
    let group = p.get("group").and_then(Value::as_str);
    let group_colors: Option<Vec<Color>> = match group {
        Some(g) => {
            let g = st.doc.swatch_groups.iter().find(|x| x.name == g).ok_or_else(|| format!("no colour group `{g}`"))?;
            Some(g.swatches.iter().filter_map(|w| w.paint.color()).collect())
        }
        None => None,
    };
    let assign: Vec<Value> = match (&group_colors, p.get("colors")) {
        (Some(cs), _) => cs.iter().map(key).collect(),
        (None, Some(Value::Array(cs))) => cs.iter().filter_map(color_value).map(|c| key(&c)).collect(),
        _ => vec![],
    };
    // Colours alone (no art, no group) become a new colour group.
    let new_group = !art && group.is_none();
    if new_group && assign.is_empty() {
        return Err("select artwork (or a colour group) to recolor".into());
    }
    let count = match p.get("colors").and_then(Value::as_u64) {
        Some(n) => json!(n.max(1)),
        None if !assign.is_empty() => json!(assign.len()),
        None => Value::Null,
    };
    let limit = match p.get("library").and_then(Value::as_str) {
        Some("") => first_library(app).unwrap_or_default(),
        Some(l) => limit_key(app, l)?,
        None => String::new(),
    };
    // Without art the rows are the group's colours, as they are.
    let start: Vec<Value> = if art { vec![] } else { assign.iter().map(|k| json!({"from": [k], "to": k})).collect() };
    // The Color Groups list starts as the document's groups (`orig`: the name each has there).
    let groups: Vec<Value> = st
        .doc
        .swatch_groups
        .iter()
        .map(|g| json!({"name": g.name, "colors": g.swatches.iter().filter_map(|w| w.paint.color()).map(|c| key(&c)).collect::<Vec<_>>(), "orig": g.name}))
        .collect();
    let orig: Vec<&str> = st.doc.swatch_groups.iter().map(|g| g.name.as_str()).collect();
    let fields = json!({
        "rows": start, "colors": count, "method": Method::ScaleTints.id(),
        "preserveWhite": true, "preserveBlack": true, "preserveGrays": false, "limitTo": limit,
        "group": group, "groupName": if new_group { Some(NEW_GROUP) } else { group }, "tab": "assign", "rule": Harmony::Complementary.id(), "linked": true,
        "preview": true, "sort": Sort::None.id(), "__art": art, "__assign": assign, "__newGroup": new_group,
        "__groups": groups, "__origGroups": orig,
    });
    let mut d = Dialog::new(KIND, fields);
    // Without art the rows are there from the start: limit them now (art's rows snap as they reduce).
    let mut rs = rows(&d);
    snap(app, &d, &mut rs);
    d.fields.insert("rows".into(), Value::Array(rs));
    app.ui.dialog = Some(d);
    Ok(Value::Null)
}

fn key(c: &Color) -> Value {
    json!(ColorKey::of(c).to_string())
}

/// The name a new colour group (opened on colours alone) starts with.
const NEW_GROUP: &str = "Color Group";

fn first_library(app: &VectorcraftApp) -> Option<String> {
    colour_libraries(app).into_iter().next().map(|l| l.id)
}

/// The field that holds the `recolor.reduce` parameters the rows were last reduced with.
const REDUCED: &str = "__reduced";

/// `recolor.reduce` parameters (Method only picks each row's colour, so changing it keeps the
/// rows): the new colours to assign are the active group's, or the opener's colours.
fn reduce_params(d: &Dialog) -> Value {
    let assign = assign(d);
    json!({
        "colors": d.fields.get("colors").cloned().unwrap_or(Value::Null),
        "preserve": {"white": d.bool("preserveWhite"), "black": d.bool("preserveBlack"), "grays": d.bool("preserveGrays")},
        "limitTo": d.str("limitTo"),
        "sort": sort(d).id(),
        "assign": if assign.is_empty() { Value::Null } else { Value::Array(assign) },
    })
}

fn sort(d: &Dialog) -> Sort {
    d.fields.get("sort").and_then(Value::as_str).and_then(Sort::parse).unwrap_or_default()
}

/// The new colours assigned to the rows: the active group's, or the opener's colours.
fn assign(d: &Dialog) -> Vec<Value> {
    d.fields.get("__assign").and_then(Value::as_array).cloned().unwrap_or_default()
}

/// Take the rows as they are as reduced with the current parameters (they were changed by hand).
fn mark_reduced(d: &mut Dialog) {
    let sig = reduce_params(d);
    d.fields.insert(REDUCED.into(), sig);
}

/// Reduce the art's colours into rows again when the count, preserve flags, library, sort or the
/// colours to assign changed (pairing those colours with the rows).
fn sync(app: &mut VectorcraftApp, d: &mut Dialog) {
    let sig = reduce_params(d);
    if !d.bool("__art") || d.fields.get(REDUCED) == Some(&sig) {
        return;
    }
    let mut p = sig.clone();
    p["method"] = json!(d.str("method"));
    match app.session.execute("recolor.reduce", &p) {
        Ok(r) => {
            let mut rows = r["map"].as_array().cloned().unwrap_or_default();
            snap(app, d, &mut rows);
            d.fields.insert("rows".into(), Value::Array(rows));
            d.fields.insert("__sel".into(), json!([]));
        }
        Err(e) => app.status(e.to_string()),
    }
    d.fields.insert(REDUCED.into(), sig);
}

/// With Limit to Library, the rows' new colours snap to the library's nearest colours.
fn snap(app: &VectorcraftApp, d: &Dialog, rows: &mut [Value]) {
    let Some(palette) = Some(d.str("limitTo")).filter(|l| !l.is_empty()).and_then(|l| swatchlib::limit_palette(&app.session, &l)) else { return };
    for r in rows {
        if let Some(c) = to_color(r) {
            r["to"] = key(&palette.nearest(c));
        }
    }
}

fn rows(d: &Dialog) -> Vec<Value> {
    d.fields.get("rows").and_then(Value::as_array).cloned().unwrap_or_default()
}

fn from_colors(row: &Value) -> Vec<Color> {
    row["from"].as_array().map(|a| a.iter().filter_map(color_value).collect()).unwrap_or_default()
}

fn to_color(row: &Value) -> Option<Color> {
    color_value(&row["to"])
}

fn excluded(row: &Value) -> bool {
    row["exclude"].as_bool().unwrap_or(false)
}

fn selection(d: &Dialog) -> Vec<usize> {
    d.fields.get("__sel").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(|i| i as usize).collect()).unwrap_or_default()
}

/// `recolor.apply` parameters: the rows, method and library; the changes to the Color Groups list
/// (`groupEdits`); and the active group with its new name and colours ([`group_colors`]).
fn apply_params(d: &Dialog) -> Value {
    let (edits, active) = group_plan(d);
    let mut p = json!({"map": rows(d), "method": d.str("method"), "limitTo": d.str("limitTo")});
    if !edits.is_empty() {
        p["groupEdits"] = Value::Array(edits);
    }
    if let Some((g, rename)) = active {
        p["group"] = json!(g);
        p["groupColors"] = Value::Array(group_colors(d));
        if let Some(r) = rename {
            p["rename"] = json!(r);
        }
    }
    p
}

/// The active group's colours after the recolour: with art, each colour a row took (its
/// `source`) replaced by that row's new colour and the others as they were; rows without a
/// source (e.g. without art) give the rows' new colours, then the group's colours no row took.
fn group_colors(d: &Dialog) -> Vec<Value> {
    let rs = rows(d);
    let mut colors = assign(d);
    if d.bool("__art") && rs.iter().any(|r| r["source"].is_u64()) {
        for r in &rs {
            if let Some(c) = r["source"].as_u64().and_then(|j| colors.get_mut(j as usize)) {
                *c = r["to"].clone();
            }
        }
        return colors;
    }
    let rest = if d.bool("__art") { colors.split_off(rs.len().min(colors.len())) } else { vec![] };
    rs.iter().map(|r| r["to"].clone()).chain(rest).collect()
}

/// The Color Groups list.
fn groups(d: &Dialog) -> Vec<Value> {
    d.fields.get("__groups").and_then(Value::as_array).cloned().unwrap_or_default()
}

fn group_name(g: &Value) -> &str {
    g["name"].as_str().unwrap_or_default()
}

/// The active group's place in the Color Groups list.
fn active_index(d: &Dialog) -> Option<usize> {
    let g = d.fields.get("group").and_then(Value::as_str)?;
    groups(d).iter().position(|x| group_name(x) == g)
}

/// The edits the Color Groups list makes to the document's groups (`recolor.apply`'s
/// `groupEdits`: groups deleted, saved with changes or new), and the active group as `recolor.apply`
/// rewrites it: its name in the document (a new group's name as it is added) and its new name.
fn group_plan(d: &Dialog) -> (Vec<Value>, Option<(String, Option<String>)>) {
    let list = groups(d);
    let active = active_index(d);
    let mut edits: Vec<Value> = vec![];
    let origs = d.fields.get("__origGroups").and_then(Value::as_array).cloned().unwrap_or_default();
    for o in origs.iter().filter_map(Value::as_str) {
        if !list.iter().any(|g| g["orig"].as_str() == Some(o)) {
            edits.push(json!({"group": o, "delete": true}));
        }
    }
    for (_, g) in list.iter().enumerate().filter(|(i, _)| Some(*i) != active) {
        match g["orig"].as_str() {
            Some(o) if g["dirty"].as_bool() == Some(true) => edits.push(json!({"group": o, "name": g["name"], "colors": g["colors"]})),
            None => edits.push(json!({"name": g["name"], "colors": g["colors"]})),
            _ => {}
        }
    }
    let active = active.and_then(|i| list.get(i)).map(|g| {
        let doc_name = match g["orig"].as_str() {
            Some(o) => o.to_string(),
            None => {
                // A new group: added empty, then rewritten with the colours like any other.
                edits.push(json!({"name": g["name"], "colors": []}));
                group_name(g).to_string()
            }
        };
        let name = d.fields.get("groupName").and_then(Value::as_str).unwrap_or_default().to_string();
        let rename = (!name.trim().is_empty() && name != doc_name).then_some(name);
        (doc_name, rename)
    });
    (edits, active)
}

/// What the buttons and the list of the Color Groups column do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GroupAction {
    /// Make the group at this place in the list the active group: its colours become the rows' new
    /// colours.
    Pick(usize),
    /// Back to the art's own colours, without an active group.
    FromArt,
    /// A new group of the rows' new colours (excluded rows left out), made the active group.
    New,
    /// The active group takes the new colours (and the Color Group name).
    Save,
    /// Remove the active group from the list.
    Delete,
}

pub(crate) fn group_action(app: &VectorcraftApp, d: &mut Dialog, a: GroupAction) {
    let mut list = groups(d);
    let active = active_index(d);
    match a {
        GroupAction::Pick(i) => {
            let Some(g) = list.get(i) else { return };
            let colors = g["colors"].as_array().cloned().unwrap_or_default();
            d.fields.insert("group".into(), g["name"].clone());
            d.fields.insert("groupName".into(), g["name"].clone());
            d.fields.insert("__newGroup".into(), json!(false));
            if d.bool("__art") {
                // As many rows as the group has colours; `sync` reduces and pairs them.
                d.fields.insert("colors".into(), json!(colors.len().max(1)));
                d.fields.insert("__assign".into(), Value::Array(colors));
                d.fields.remove(REDUCED);
            } else {
                let mut rs: Vec<Value> = colors.iter().map(|k| json!({"from": [k], "to": k})).collect();
                snap(app, d, &mut rs);
                d.fields.insert("__assign".into(), Value::Array(colors));
                d.fields.insert("rows".into(), Value::Array(rs));
            }
            d.fields.insert("__sel".into(), json!([]));
        }
        GroupAction::FromArt => {
            for k in ["group", "groupName", "colors"] {
                d.fields.insert(k.into(), Value::Null);
            }
            d.fields.insert("__assign".into(), json!([]));
            d.fields.remove(REDUCED);
        }
        GroupAction::New => {
            let mut rs = rows(d);
            let mut colors = vec![];
            for r in rs.iter_mut() {
                if excluded(r) || r["to"].is_null() {
                    if let Some(o) = r.as_object_mut() {
                        o.remove("source");
                    }
                } else {
                    r["source"] = json!(colors.len());
                    colors.push(r["to"].clone());
                }
            }
            let name = free_group_name(app, &list, NEW_GROUP);
            list.push(json!({"name": name, "colors": colors, "orig": null}));
            d.fields.insert("rows".into(), Value::Array(rs));
            d.fields.insert("__groups".into(), Value::Array(list));
            d.fields.insert("group".into(), json!(name));
            d.fields.insert("groupName".into(), json!(name));
            d.fields.insert("__newGroup".into(), json!(false));
            d.fields.insert("__assign".into(), Value::Array(colors));
            mark_reduced(d);
        }
        GroupAction::Save => {
            let Some(i) = active else { return };
            let colors = group_colors(d);
            let Some(g) = list.get_mut(i) else { return };
            let name =
                d.fields.get("groupName").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).unwrap_or(group_name(g)).to_string();
            g["name"] = json!(name);
            g["colors"] = Value::Array(colors.clone());
            g["dirty"] = json!(true);
            d.fields.insert("__groups".into(), Value::Array(list));
            d.fields.insert("group".into(), json!(name));
            d.fields.insert("groupName".into(), json!(name));
            d.fields.insert("__assign".into(), Value::Array(colors));
            mark_reduced(d);
        }
        GroupAction::Delete => {
            let Some(i) = active else { return };
            list.remove(i);
            d.fields.insert("__groups".into(), Value::Array(list));
            d.fields.insert("group".into(), Value::Null);
            d.fields.insert("groupName".into(), Value::Null);
        }
    }
}

/// `base`, or "base 2", "base 3"… unused by the list and the document's swatches.
fn free_group_name(app: &VectorcraftApp, list: &[Value], base: &str) -> String {
    let taken = |n: &str| list.iter().any(|g| group_name(g) == n) || app.session.active().is_some_and(|st| st.doc.swatch_name_taken(n));
    std::iter::once(base.to_string()).chain((2..1000).map(|i| format!("{base} {i}"))).find(|n| !taken(n)).unwrap_or_else(|| base.to_string())
}

fn body(app: &mut VectorcraftApp, ui: &mut Ui, d: &mut Dialog) -> bool {
    sync(app, d);
    let t = Tokens::get(ui.ctx());
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(LEFT_W);
            tabs(app, ui, d);
        });
        ui.add_space(8.0);
        ui.separator();
        ui.vertical(|ui| {
            ui.set_width(GROUPS_W);
            groups_column(app, ui, d);
        });
    });
    if d.fields.get("group").is_some_and(Value::is_string) || d.bool("__newGroup") {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tl!("Color Group:")).color(t.text_dim));
            form::text(ui, d, "groupName", 200.0);
        });
    }
    sync(app, d);
    if d.bool("__art") {
        form::preview(app, ui, d, "Recolor Artwork", CMD, apply_params(d));
    }
    false
}

/// The Color Groups column: the list (click a group to make it the active group) and its buttons.
fn groups_column(app: &mut VectorcraftApp, ui: &mut Ui, d: &mut Dialog) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(2.0);
    ui.label(egui::RichText::new(tl!("Color Groups")).font(theme::semibold(12.5)));
    ui.add_space(6.0);
    let list = groups(d);
    let active = active_index(d);
    let mut action = None;
    widgets::list_box(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("recolor-groups").max_height(TAB_H - 70.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(TAB_H - 70.0);
            for (i, g) in list.iter().enumerate() {
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), GROUP_ROW_H), Sense::hover());
                let resp = ui.interact(r, egui::Id::new(("recolor-group", i)), Sense::click());
                if Some(i) == active {
                    ui.painter().rect_filled(r, 2.0, t.row_selected);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, 2.0, t.hover);
                }
                // Group names are the user's: shown as they are.
                let font = egui::FontId::proportional(11.5);
                ui.painter().text(r.left_top() + vec2(6.0, 3.0), egui::Align2::LEFT_TOP, group_name(g), font, t.text);
                let colors: Vec<Color> = g["colors"].as_array().map(|a| a.iter().filter_map(color_value).collect()).unwrap_or_default();
                let fit = ((r.width() - 12.0) / (GROUP_CHIP + 2.0)).max(1.0) as usize;
                for (j, c) in colors.iter().take(fit).enumerate() {
                    let x = r.left() + 6.0 + j as f32 * (GROUP_CHIP + 2.0);
                    chip(ui, Rect::from_min_size(pos2(x, r.bottom() - GROUP_CHIP - 4.0), vec2(GROUP_CHIP, GROUP_CHIP)), *c, false);
                }
                if resp.on_hover_text(group_name(g)).clicked() {
                    action = Some(GroupAction::Pick(i));
                }
            }
        });
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let (art, has) = (d.bool("__art"), active.is_some());
        let buttons = [
            ("pipette", tl!("Get Colors from Selected Art"), art, GroupAction::FromArt),
            ("dc-folder", tl!("New Color Group"), true, GroupAction::New),
            ("save", tl!("Save Changes to Color Group"), has, GroupAction::Save),
            ("trash-2", tl!("Delete Color Group"), has, GroupAction::Delete),
        ];
        for (icon, tip, on, a) in buttons {
            if widgets::icon_button_enabled(ui, icon, tip, false, on, 24.0).clicked() {
                action = Some(a);
            }
        }
    });
    if let Some(a) = action {
        group_action(app, d, a);
    }
}

/// The Assign and Edit tabs, with the preset menu above them.
fn tabs(app: &mut VectorcraftApp, ui: &mut Ui, d: &mut Dialog) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        let tab = d.str("tab");
        for (id, label) in [("assign", tl!("Assign")), ("edit", tl!("Edit"))] {
            if ui.selectable_label(tab == id || (id == "assign" && tab != "edit"), egui::RichText::new(label).font(theme::semibold(12.5))).clicked() {
                d.fields.insert("tab".into(), json!(id));
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(i) = widgets::dropdown(ui, "recolor-preset", PRESETS[preset_of(d)], &PRESETS, 130.0) {
                set_preset(app, d, i);
            }
            ui.label(egui::RichText::new(tl!("Preset:")).color(t.text_dim));
        });
    });
    widgets::divider(ui);
    ui.add_space(4.0);
    let mut rs = rows(d);
    let top = ui.cursor().top();
    let changed = if d.str("tab") == "edit" { edit_tab(ui, d, &mut rs) } else { assign_tab(app, ui, d, &mut rs) };
    ui.add_space((TAB_H - (ui.cursor().top() - top)).max(0.0));
    if changed {
        snap(app, d, &mut rs);
        d.fields.insert("rows".into(), Value::Array(rs));
    }
}

/// OK: keep the previewed recolour as one undo step (or apply it, e.g. to a colour group only; or
/// save colours opened alone as a new colour group).
fn confirm(app: &mut VectorcraftApp, d: &Dialog) -> Result<Value, String> {
    let mut d = d.clone();
    sync(app, &mut d);
    if d.bool("__newGroup") {
        let colors: Vec<Value> = rows(&d).into_iter().filter(|r| !excluded(r)).map(|r| r["to"].clone()).collect();
        let new = json!({"name": d.str("groupName"), "colors": colors});
        let (mut edits, _) = group_plan(&d);
        if edits.is_empty() {
            return form::commit_preview(app, "swatch.newGroup", new);
        }
        // With changes to the Color Groups list, the new group is made with them.
        edits.push(new);
        return form::commit_preview(app, CMD, json!({"map": [], "groupEdits": edits}));
    }
    form::commit_preview(app, CMD, apply_params(&d))
}

/// The preset the fields match: an n-colour job (n rows, Scale Tints), a library, or Custom.
fn preset_of(d: &Dialog) -> usize {
    match d.fields.get("colors").and_then(Value::as_u64) {
        Some(n @ 1..=3) if d.str("method") == Method::ScaleTints.id() => n as usize,
        _ if !d.str("limitTo").is_empty() => 4,
        _ => 0,
    }
}

fn set_preset(app: &VectorcraftApp, d: &mut Dialog, i: usize) {
    match i {
        1..=3 => {
            d.fields.insert("colors".into(), json!(i));
            d.fields.insert("method".into(), json!(Method::ScaleTints.id()));
        }
        4 if d.str("limitTo").is_empty() => {
            d.fields.insert("limitTo".into(), json!(first_library(app).unwrap_or_default()));
        }
        _ => {}
    }
}

/// A colour chip with a hairline frame.
fn chip(ui: &Ui, r: Rect, c: Color, dim: bool) {
    let t = Tokens::get(ui.ctx());
    let fill = c32(&c);
    ui.painter().rect_filled(r, 1.0, if dim { fill.gamma_multiply(0.35) } else { fill });
    ui.painter().rect_stroke(r, 1.0, egui::Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
}

/// The Assign tab. Returns true when the rows changed.
fn assign_tab(app: &mut VectorcraftApp, ui: &mut Ui, d: &mut Dialog, rows: &mut Vec<Value>) -> bool {
    let t = Tokens::get(ui.ctx());
    let mut changed = false;
    let total: usize = rows.iter().map(|r| r["from"].as_array().map_or(0, Vec::len)).sum();
    // Reduction and the preserve rules apply to art (a colour group alone has a row per colour).
    if d.bool("__art") {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tl!("Colors:")).color(t.text_dim));
            let labels: Vec<String> = std::iter::once(tl!("Auto").to_string()).chain((1..=total).map(|n| n.to_string())).collect();
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            let cur = d.fields.get("colors").and_then(Value::as_u64).map_or(tl!("Auto").to_string(), |n| n.to_string());
            if let Some(i) = widgets::dropdown(ui, "recolor-count", &cur, &refs, 70.0) {
                d.fields.insert("colors".into(), if i == 0 { Value::Null } else { json!(i) });
            }
            ui.add_space(18.0);
            ui.label(egui::RichText::new(tl!("Preserve:")).color(t.text_dim));
            for (k, label) in [("preserveWhite", tl!("White")), ("preserveBlack", tl!("Black")), ("preserveGrays", tl!("Grays"))] {
                let on = d.bool(k);
                if widgets::check(ui, label, on, true) {
                    d.fields.insert(k.into(), json!(!on));
                }
            }
        });
        ui.add_space(6.0);
    }
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.add_sized(
            [CURRENT_W, 16.0],
            egui::Label::new(
                egui::RichText::new(crate::i18n::fmt(tl!("Current Colors ({total})"), &[("total", &total.to_string())])).color(t.text_dim),
            ),
        );
        ui.add_space(60.0);
        ui.label(egui::RichText::new(tl!("New")).color(t.text_dim));
    });
    let mut sel = selection(d);
    let mut clicked: Option<usize> = None;
    widgets::list_box(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("recolor-rows").max_height(TABLE_H).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(TABLE_H);
            if rows.is_empty() {
                ui.add_space(8.0);
                widgets::dim_label(ui, tl!("No colors to recolor (preserved colors are left out)."));
            }
            for (i, row) in rows.iter_mut().enumerate() {
                let bg = ui.painter().add(egui::Shape::Noop);
                let off = excluded(row);
                let r = ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    let from = from_colors(row);
                    let (cur, cr) = ui.allocate_exact_size(vec2(CURRENT_W, 20.0), Sense::click());
                    let w = CURRENT_W / from.len().max(1) as f32;
                    for (j, c) in from.iter().enumerate() {
                        let x = cur.left() + j as f32 * w;
                        chip(ui, Rect::from_min_max(pos2(x, cur.top()), pos2(x + w - if w > 4.0 { 2.0 } else { 0.0 }, cur.bottom())), *c, false);
                    }
                    if cr.clicked() {
                        clicked = Some(i);
                    }
                    let (ar, aresp) = ui.allocate_exact_size(vec2(44.0, 20.0), Sense::click());
                    arrow(ui, ar, !off, aresp.hovered());
                    if aresp.on_hover_text(if off { tl!("Recolor this row") } else { tl!("Keep this row's colors (exclude)") }).clicked() {
                        row["exclude"] = json!(!off);
                        changed = true;
                    }
                    let (nr, nresp) = ui.allocate_exact_size(vec2(46.0, 20.0), Sense::click());
                    let to = to_color(row).unwrap_or_default();
                    chip(ui, nr, to, off);
                    if nresp.clicked() {
                        clicked = Some(i);
                    }
                    ui.add_space(6.0);
                    let hex = crate::panels::color::hex_digits(&to);
                    if let Some(h) = widgets::hex_field(ui, ("recolor-hex", i), &hex)
                        && let Some(c) = crate::panels::color::parse_hex(&h)
                    {
                        row["to"] = key(&keep_model(to, c));
                        changed = true;
                    }
                });
                if sel.contains(&i) {
                    ui.painter().set(bg, egui::Shape::rect_filled(r.response.rect.expand2(vec2(2.0, 1.0)), 2.0, t.row_selected));
                }
            }
        });
    });
    if let Some(i) = clicked {
        let m = ui.input(|i| i.modifiers);
        match sel.iter().position(|s| *s == i) {
            Some(p) if m.shift || m.command => {
                sel.remove(p);
            }
            None if m.shift || m.command => sel.push(i),
            _ => sel = vec![i],
        }
        d.fields.insert("__sel".into(), json!(sel));
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let can_merge = sel.len() > 1;
        if widgets::icon_button_enabled(ui, "combine", tl!("Merge colors into one row"), false, can_merge, 24.0).clicked() {
            merge(rows, &sel);
            d.fields.insert("__sel".into(), json!([sel.iter().min()]));
            changed = true;
        }
        let can_split = sel.iter().any(|i| rows.get(*i).is_some_and(|r| r["from"].as_array().is_some_and(|a| a.len() > 1)));
        if widgets::icon_button_enabled(ui, "ungroup", tl!("Separate colors into rows"), false, can_split, 24.0).clicked() {
            separate(rows, &sel);
            d.fields.insert("__sel".into(), json!([]));
            changed = true;
        }
        ui.add_space(12.0);
        for (icon, tip, sb) in
            [("arrow-left-right", tl!("Randomly change color order"), false), ("sparkles", tl!("Randomly change saturation and brightness"), true)]
        {
            if widgets::icon_button_enabled(ui, icon, tip, false, rows.len() > 1 || sb, 24.0).clicked() {
                let seed = d.f64("__seed", 0.0) as u64 + 1;
                d.fields.insert("__seed".into(), json!(seed));
                let p = json!({"map": rows, "order": !sb, "saturationBrightness": sb, "seed": seed});
                match app.session.execute("recolor.randomize", &p) {
                    Ok(r) => {
                        // The rows keep the group colour each took.
                        let old = std::mem::replace(rows, r["map"].as_array().cloned().unwrap_or_default());
                        for (n, o) in rows.iter_mut().zip(&old) {
                            if let Some(src) = o.get("source").filter(|v| !v.is_null()) {
                                n["source"] = src.clone();
                            }
                        }
                        changed = true;
                    }
                    Err(e) => app.status(e.to_string()),
                }
            }
        }
        if widgets::icon_button(ui, "rotate-ccw", tl!("Reset the rows to the artwork's colors"), false, 24.0).clicked() {
            d.fields.remove(REDUCED);
        }
    });
    ui.add_space(8.0);
    egui::Grid::new("recolor-options").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
        ui.label(egui::RichText::new(tl!("Method:")).color(t.text_dim));
        let m = Method::parse(&d.str("method")).unwrap_or_default();
        let labels: Vec<&str> = Method::ALL.iter().map(|m| m.label()).collect();
        if let Some(i) = widgets::dropdown(ui, "recolor-method", m.label(), &labels, 180.0) {
            d.fields.insert("method".into(), json!(Method::ALL[i].id()));
        }
        ui.end_row();
        // The rows' order, and how a group's colours pair with them.
        if d.bool("__art") {
            ui.label(egui::RichText::new(tl!("Sort:")).color(t.text_dim));
            let labels: Vec<&str> = Sort::ALL.iter().map(|s| s.label()).collect();
            if let Some(i) = widgets::dropdown(ui, "recolor-sort", sort(d).label(), &labels, 180.0) {
                d.fields.insert("sort".into(), json!(Sort::ALL[i].id()));
            }
            ui.end_row();
        }
        ui.label(egui::RichText::new(tl!("Limit to Library:")).color(t.text_dim));
        // None, the document's swatches, then the colour libraries: the built-in ones (at the top
        // level of the library menu) translated, those the user saved or loaded by their names.
        let libs = colour_libraries(app);
        let keys: Vec<&str> = ["", swatchlib::DOCUMENT_SWATCHES].into_iter().chain(libs.iter().map(|l| l.id.as_str())).collect();
        let names: Vec<&str> = ["None", DOCUMENT_SWATCHES].into_iter().chain(libs.iter().map(|l| l.name.as_str())).collect();
        let builtin = |k: usize| k.checked_sub(2).is_none_or(|i| libs.get(i).is_some_and(|l| l.submenu.is_none()));
        let shown = super::shown_names(crate::i18n::current(), &names, builtin);
        let cur = d.str("limitTo");
        let at = keys.iter().position(|k| *k == cur).unwrap_or(0);
        let current = shown.get(at).copied().unwrap_or_default();
        if let Some(k) = widgets::dropdown_names(ui, "recolor-library", current, &shown, 180.0).and_then(|i| keys.get(i)) {
            d.fields.insert("limitTo".into(), json!(k));
        }
        ui.end_row();
    });
    changed
}

/// The arrow between a row's current and new colours: solid when the row is recoloured, a dashed
/// line without a head when it is excluded.
fn arrow(ui: &Ui, r: Rect, on: bool, hovered: bool) {
    let t = Tokens::get(ui.ctx());
    let c = if hovered {
        t.text_strong
    } else if on {
        t.text
    } else {
        t.text_disabled
    };
    let (a, b) = (pos2(r.left() + 6.0, r.center().y), pos2(r.right() - 6.0, r.center().y));
    let stroke = egui::Stroke::new(1.2, c);
    if on {
        ui.painter().line_segment([a, b], stroke);
        ui.painter().line_segment([b, b + vec2(-5.0, -4.0)], stroke);
        ui.painter().line_segment([b, b + vec2(-5.0, 4.0)], stroke);
    } else {
        ui.painter().extend(egui::Shape::dashed_line(&[a, b], stroke, 3.0, 3.0));
    }
}

/// Merge the selected rows into the first of them (its new colour stays).
fn merge(rows: &mut Vec<Value>, sel: &[usize]) {
    let mut sel: Vec<usize> = sel.iter().copied().filter(|i| *i < rows.len()).collect();
    sel.sort_unstable();
    let Some((&first, rest)) = sel.split_first() else { return };
    let mut moved = vec![];
    for &i in rest.iter().rev() {
        let row = rows.remove(i);
        moved.splice(0..0, row["from"].as_array().cloned().unwrap_or_default());
    }
    if let Some(dst) = rows[first]["from"].as_array_mut() {
        dst.extend(moved);
    }
}

/// Split the selected rows into one row per colour, each its own new colour.
fn separate(rows: &mut Vec<Value>, sel: &[usize]) {
    let mut out = vec![];
    for (i, row) in rows.drain(..).enumerate() {
        match row["from"].as_array() {
            Some(a) if sel.contains(&i) && a.len() > 1 => out.extend(a.iter().map(|k| json!({"from": [k], "to": k}))),
            _ => out.push(row),
        }
    }
    *rows = out;
}

/// The rows the Edit tab shows: those with a new colour, not excluded.
fn active_rows(rows: &[Value]) -> Vec<usize> {
    (0..rows.len()).filter(|i| !excluded(&rows[*i]) && to_color(&rows[*i]).is_some()).collect()
}

/// Set row `i`'s new colour to hue/saturation/brightness `hsb` ([`move_on_wheel`]: with `linked`
/// the other rows keep their relation to it). Excluded rows stay.
fn move_color(rows: &mut [Value], i: usize, hsb: [f32; 3], linked: bool) {
    let active = active_rows(rows);
    let Some(k) = active.iter().position(|&j| j == i) else { return };
    let before: Vec<Color> = active.iter().filter_map(|&j| to_color(&rows[j])).collect();
    let mut colors = before.clone();
    move_on_wheel(&mut colors, k, hsb, linked);
    for ((&j, c), old) in active.iter().zip(&colors).zip(&before) {
        if c != old {
            rows[j]["to"] = key(c);
        }
    }
}

/// The Edit tab. Returns true when the rows changed.
fn edit_tab(ui: &mut Ui, d: &mut Dialog, rows: &mut [Value]) -> bool {
    let t = Tokens::get(ui.ctx());
    let mut changed = false;
    let linked = d.bool("linked");
    let active = active_rows(rows);
    let base = selection(d).into_iter().find(|i| active.contains(i)).or(active.first().copied());
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(tl!("Harmony Rules:")).color(t.text_dim));
        let rule = Harmony::parse(&d.str("rule")).unwrap_or(Harmony::Complementary);
        let labels: Vec<&str> = Harmony::ALL.iter().map(|h| h.label()).collect();
        // Choosing a rule gives the new colours its colours, from the base colour.
        if let Some(i) = widgets::dropdown(ui, "recolor-rule", rule.label(), &labels, 170.0) {
            d.fields.insert("rule".into(), json!(Harmony::ALL[i].id()));
            if let Some((b, c)) = base.and_then(|b| Some((b, to_color(&rows[b])?))) {
                apply_rule(rows, Harmony::ALL[i], b, c, &active);
                changed = true;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let tip = if linked { tl!("Unlink harmony colors") } else { tl!("Link harmony colors") };
            if widgets::icon_button(ui, if linked { "link" } else { "link-2-off" }, tip, linked, 24.0).clicked() {
                d.fields.insert("linked".into(), json!(!linked));
            }
        });
    });
    ui.add_space(8.0);
    let colors: Vec<Color> = active.iter().filter_map(|&i| to_color(&rows[i])).collect();
    let w = widgets::harmony_wheel(ui, "recolor-wheel", WHEEL, &colors, base.and_then(|b| active.iter().position(|&i| i == b)));
    if let Some(k) = w.pressed {
        d.fields.insert("__sel".into(), json!([active[k]]));
    }
    if let Some((k, hsb)) = w.moved {
        move_color(rows, active[k], hsb, linked);
        changed = true;
    }
    ui.add_space(6.0);
    // The new colours: click one to make it the base colour.
    ui.horizontal_wrapped(|ui| {
        for &i in &active {
            let (r, resp) = ui.allocate_exact_size(vec2(28.0, 20.0), Sense::click());
            chip(ui, r, to_color(&rows[i]).unwrap_or_default(), false);
            if Some(i) == base {
                ui.painter().rect_stroke(r.expand(1.5), 2.0, egui::Stroke::new(1.5, t.accent), egui::StrokeKind::Outside);
            }
            if resp.on_hover_text(rows[i]["to"].as_str().unwrap_or_default()).clicked() {
                d.fields.insert("__sel".into(), json!([i]));
            }
        }
    });
    changed
}

/// Give the rows `active` the colours of `rule` from the base colour `b` of row `base` (the base row
/// first, then the others in order, the rule's colours repeating), each in its row's model.
fn apply_rule(rows: &mut [Value], rule: Harmony, base: usize, b: Color, active: &[usize]) {
    let colors = rule.apply(b);
    let order = std::iter::once(base).chain(active.iter().copied().filter(|i| *i != base));
    for (n, i) in order.enumerate() {
        let old = to_color(&rows[i]).unwrap_or(b);
        rows[i]["to"] = key(&keep_model(old, colors[n % colors.len()]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<Value> {
        vec![
            json!({"from": ["rgb 255 0 0"], "to": "rgb 255 0 0"}),
            json!({"from": ["rgb 0 0 255", "rgb 0 0 128"], "to": "rgb 0 0 255"}),
            json!({"from": ["rgb 0 255 0"], "to": "rgb 0 255 0"}),
        ]
    }

    #[test]
    fn merge_and_separate_rows() {
        let mut r = rows();
        merge(&mut r, &[2, 0]);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["from"], json!(["rgb 255 0 0", "rgb 0 255 0"]));
        assert_eq!(r[0]["to"], "rgb 255 0 0", "the first row's new colour stays");
        separate(&mut r, &[1]);
        assert_eq!(r.len(), 3);
        assert_eq!(r[1], json!({"from": ["rgb 0 0 255"], "to": "rgb 0 0 255"}));
        assert_eq!(r[2]["to"], "rgb 0 0 128");
    }

    #[test]
    fn linked_handles_move_together() {
        let mut r = rows();
        move_color(&mut r, 0, [120.0, 1.0, 1.0], false);
        assert_eq!(r[0]["to"], "rgb 0 255 0");
        assert_eq!(r[1]["to"], "rgb 0 0 255", "unlinked: only that colour");
        let mut r = rows();
        move_color(&mut r, 0, [120.0, 1.0, 1.0], true);
        assert_eq!(r[1]["to"], "rgb 255 0 0", "linked: every colour turns by the same hue");
        assert_eq!(r[2]["to"], "rgb 0 0 255");
        // Excluded rows stay.
        let mut r = rows();
        r[2]["exclude"] = json!(true);
        move_color(&mut r, 0, [120.0, 1.0, 1.0], true);
        assert_eq!(r[2]["to"], "rgb 0 255 0");
    }

    #[test]
    fn harmony_rules_fill_the_rows_from_the_base() {
        let mut r = rows();
        apply_rule(&mut r, Harmony::Triad, 0, Color::rgb(1.0, 0.0, 0.0), &[0, 1, 2]);
        assert_eq!([&r[0]["to"], &r[1]["to"], &r[2]["to"]], ["rgb 255 0 0", "rgb 0 255 0", "rgb 0 0 255"]);
    }
}
