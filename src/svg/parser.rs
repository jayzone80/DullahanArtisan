/// SVG DOM parser, style resolver, gradient/clip/filter definitions, and strict Game-UI validator.

use std::collections::HashMap;
use std::path::Path;

use crate::svg::path::{
    ellipse_path, parse_number_list, parse_svg_path, parse_transform, rect_path, Affine, BBox,
    LineCap, LineJoin, PathData, Pt, StrokeStyle, Subpath,
};
use crate::svg::xml::{parse_xml, XmlNode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillRule {
    NonZero,
    EvenOdd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpreadMethod {
    Pad,
    Reflect,
    Repeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordUnits {
    ObjectBoundingBox,
    UserSpaceOnUse,
}

#[derive(Debug, Clone)]
pub struct GradientStop {
    pub offset: f64,
    pub color: [u8; 4], // straight RGBA with stop-opacity baked into alpha
}

#[derive(Debug, Clone)]
pub enum GradientKind {
    Linear { x1: f64, y1: f64, x2: f64, y2: f64 },
    Radial { cx: f64, cy: f64, r: f64, fx: f64, fy: f64 },
}

#[derive(Debug, Clone)]
pub struct Gradient {
    pub kind: GradientKind,
    pub units: CoordUnits,
    pub spread: SpreadMethod,
    pub transform: Affine,
    pub stops: Vec<GradientStop>,
}

#[derive(Debug, Clone)]
pub struct ClipPathDef {
    pub units: CoordUnits,
    pub shapes: Vec<(PathData, FillRule)>,
}

#[derive(Debug, Clone)]
pub enum FilterKind {
    GaussianBlur { std_x: f64, std_y: f64, merge_source: bool },
    DropShadow { dx: f64, dy: f64, std_x: f64, std_y: f64, color: [u8; 4] },
}

#[derive(Debug, Clone)]
pub enum Paint {
    None,
    Color([u8; 4]),
    Gradient(String),
}

#[derive(Debug, Clone)]
pub struct DrawShape {
    pub path: PathData,
    pub transform: Affine,
    pub fill: Paint,
    pub fill_opacity: f64,
    pub fill_rule: FillRule,
    pub stroke: Paint,
    pub stroke_opacity: f64,
    pub stroke_style: StrokeStyle,
    pub opacity: f64,
    pub clip_path: Option<String>,
    pub filter: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DrawGroup {
    pub items: Vec<RenderItem>,
    pub opacity: f64,
    pub clip_path: Option<String>,
    pub filter: Option<String>,
    pub transform: Affine,
}

#[derive(Debug, Clone)]
pub enum RenderItem {
    Shape(DrawShape),
    Group(DrawGroup),
}

#[derive(Debug, Clone)]
pub struct SvgStats {
    pub elements: usize,
    pub shapes: usize,
    pub gradients: usize,
    pub clip_paths: usize,
    pub filters: usize,
    pub content_bbox: BBox,
}

#[derive(Debug, Clone)]
pub struct SvgDocument {
    pub name: String,
    pub view_box: (f64, f64, f64, f64), // (min_x, min_y, width, height)
    pub width: f64,
    pub height: f64,
    pub gradients: HashMap<String, Gradient>,
    pub clip_paths: HashMap<String, ClipPathDef>,
    pub filters: HashMap<String, FilterKind>,
    pub items: Vec<RenderItem>,
    pub stats: SvgStats,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

impl SvgDocument {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

#[derive(Debug, Clone)]
struct InheritedStyle {
    fill: Paint,
    fill_opacity: f64,
    fill_rule: FillRule,
    stroke: Paint,
    stroke_opacity: f64,
    stroke_style: StrokeStyle,
    color: [u8; 4],
    visible: bool,
}

impl Default for InheritedStyle {
    fn default() -> Self {
        Self {
            fill: Paint::Color([0, 0, 0, 255]),
            fill_opacity: 1.0,
            fill_rule: FillRule::NonZero,
            stroke: Paint::None,
            stroke_opacity: 1.0,
            stroke_style: StrokeStyle::default(),
            color: [0, 0, 0, 255],
            visible: true,
        }
    }
}

struct RawGradient {
    is_radial: bool,
    href: Option<String>,
    x1: Option<f64>,
    y1: Option<f64>,
    x2: Option<f64>,
    y2: Option<f64>,
    cx: Option<f64>,
    cy: Option<f64>,
    r: Option<f64>,
    fx: Option<f64>,
    fy: Option<f64>,
    units: Option<CoordUnits>,
    spread: Option<SpreadMethod>,
    transform: Option<Affine>,
    stops: Vec<GradientStop>,
}

pub fn load_svg_file(path: &Path) -> Result<SvgDocument, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("svg")
        .to_string();
    parse_svg(&text, &name)
}

pub fn parse_svg(src: &str, name: &str) -> Result<SvgDocument, String> {
    let root = parse_xml(src)?;
    if strip_ns(&root.tag) != "svg" {
        return Err(format!("expected <svg> root element, got <{}>", root.tag));
    }

    let mut warnings = Vec::new();
    let mut errors = Vec::new();

    let raw_w = root.attr("width").and_then(parse_length);
    let raw_h = root.attr("height").and_then(parse_length);
    let vb = if let Some(vb_str) = root.attr("viewBox") {
        let nums = parse_number_list(vb_str)
            .map_err(|e| format!("invalid viewBox '{vb_str}': {e}"))?;
        if nums.len() != 4 || nums[2] <= 0.0 || nums[3] <= 0.0 {
            return Err(format!(
                "invalid viewBox '{vb_str}' (expected 4 numbers with positive width/height)"
            ));
        }
        (nums[0], nums[1], nums[2], nums[3])
    } else if let (Some(w), Some(h)) = (raw_w, raw_h) {
        if w <= 0.0 || h <= 0.0 {
            return Err("SVG width and height must be positive".into());
        }
        warnings.push("missing viewBox attribute on <svg> (derived from width/height)".into());
        (0.0, 0.0, w, h)
    } else {
        return Err("<svg> must define viewBox or positive width & height".into());
    };

    let width = raw_w.filter(|&v| v > 0.0).unwrap_or(vb.2);
    let height = raw_h.filter(|&v| v > 0.0).unwrap_or(vb.3);

    // Flat curve tolerance scaled to viewBox size so curves stay silky at any coordinate scale
    let tol = (vb.2.max(vb.3) / 2000.0).clamp(0.05, 0.5);

    // Pass 1: collect all elements by id, gradients, clipPaths, filters
    let mut nodes_by_id: HashMap<String, &XmlNode> = HashMap::new();
    let mut raw_grads: HashMap<String, RawGradient> = HashMap::new();
    let mut clip_paths: HashMap<String, ClipPathDef> = HashMap::new();
    let mut filters: HashMap<String, FilterKind> = HashMap::new();

    collect_defs(
        &root,
        tol,
        &mut nodes_by_id,
        &mut raw_grads,
        &mut clip_paths,
        &mut filters,
        &mut warnings,
        &mut errors,
    );

    // Resolve gradient inheritance (href / xlink:href)
    let mut gradients: HashMap<String, Gradient> = HashMap::new();
    for id in raw_grads.keys() {
        if let Some(g) = resolve_gradient(id, &raw_grads, 0) {
            gradients.insert(id.clone(), g);
        }
    }

    // Pass 2: build render tree
    let mut stats = SvgStats {
        elements: 0,
        shapes: 0,
        gradients: gradients.len(),
        clip_paths: clip_paths.len(),
        filters: filters.len(),
        content_bbox: BBox::empty(),
    };

    let mut ctx = BuildCtx {
        tol,
        nodes_by_id: &nodes_by_id,
        gradients: &gradients,
        clip_paths: &clip_paths,
        filters: &filters,
        stats: &mut stats,
        warnings: &mut warnings,
        errors: &mut errors,
    };

    let base_style = resolve_style(&root, &InheritedStyle::default(), &mut ctx.errors);
    let mut items = Vec::new();
    for ch in &root.children {
        ctx.visit_node(ch, &base_style, Affine::IDENTITY, 0, &mut items);
    }

    // Check viewBox overflow (warn if content extends >1.5 units outside viewBox)
    if stats.content_bbox.is_valid() {
        let margin = (vb.2.max(vb.3) * 0.005).max(1.5);
        let over_l = (vb.0 - stats.content_bbox.min_x).max(0.0);
        let over_t = (vb.1 - stats.content_bbox.min_y).max(0.0);
        let over_r = (stats.content_bbox.max_x - (vb.0 + vb.2)).max(0.0);
        let over_b = (stats.content_bbox.max_y - (vb.1 + vb.3)).max(0.0);
        let max_over = over_l.max(over_t).max(over_r).max(over_b);
        if max_over > margin {
            warnings.push(format!(
                "geometry extends {:.1}px outside viewBox (bbox=[{:.1},{:.1}..{:.1},{:.1}] vs viewBox=[{:.1},{:.1}..{:.1},{:.1}])",
                max_over,
                stats.content_bbox.min_x,
                stats.content_bbox.min_y,
                stats.content_bbox.max_x,
                stats.content_bbox.max_y,
                vb.0,
                vb.1,
                vb.0 + vb.2,
                vb.1 + vb.3
            ));
        }
    } else if stats.shapes == 0 {
        warnings.push("SVG contains no renderable vector shapes".into());
    }

    Ok(SvgDocument {
        name: name.to_string(),
        view_box: vb,
        width,
        height,
        gradients,
        clip_paths,
        filters,
        items,
        stats,
        warnings,
        errors,
    })
}

fn strip_ns(tag: &str) -> &str {
    tag.rsplit(':').next().unwrap_or(tag)
}

fn get_href(node: &XmlNode) -> Option<&str> {
    node.attr("href").or_else(|| node.attr("xlink:href"))
}

fn parse_url_ref(s: &str) -> Option<String> {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("url(").and_then(|r| r.strip_suffix(')')) {
        let inner = rest.trim().trim_matches('"').trim_matches('\'').trim();
        return inner.strip_prefix('#').map(|id| id.to_string());
    }
    None
}

fn collect_defs<'a>(
    node: &'a XmlNode,
    tol: f64,
    nodes_by_id: &mut HashMap<String, &'a XmlNode>,
    raw_grads: &mut HashMap<String, RawGradient>,
    clip_paths: &mut HashMap<String, ClipPathDef>,
    filters: &mut HashMap<String, FilterKind>,
    warnings: &mut Vec<String>,
    errors: &mut Vec<String>,
) {
    if let Some(id) = node.attr("id") {
        nodes_by_id.insert(id.to_string(), node);
    }

    let tag = strip_ns(&node.tag);
    match tag {
        "linearGradient" | "radialGradient" => {
            if let Some(id) = node.attr("id") {
                let is_radial = tag == "radialGradient";
                let units = node.attr("gradientUnits").map(|u| match u.trim() {
                    "userSpaceOnUse" => CoordUnits::UserSpaceOnUse,
                    _ => CoordUnits::ObjectBoundingBox,
                });
                let spread = node.attr("spreadMethod").map(|s| match s.trim() {
                    "reflect" => SpreadMethod::Reflect,
                    "repeat" => SpreadMethod::Repeat,
                    _ => SpreadMethod::Pad,
                });
                let transform = match node.attr("gradientTransform") {
                    Some(ts) => match parse_transform(ts) {
                        Ok(m) => Some(m),
                        Err(e) => {
                            errors.push(format!("line {}: {e}", node.line));
                            None
                        }
                    },
                    None => None,
                };
                let mut stops = Vec::new();
                for ch in &node.children {
                    if strip_ns(&ch.tag) == "stop" {
                        let style_map = parse_inline_style(ch.attr("style").unwrap_or(""));
                        let off_s = ch.attr("offset").unwrap_or("0");
                        let offset = parse_percent_or_num(off_s).unwrap_or(0.0).clamp(0.0, 1.0);
                        let col_s = style_map
                            .get("stop-color")
                            .map(|s| s.as_str())
                            .or_else(|| ch.attr("stop-color"))
                            .unwrap_or("#000000");
                        let op_s = style_map
                            .get("stop-opacity")
                            .map(|s| s.as_str())
                            .or_else(|| ch.attr("stop-opacity"));
                        let stop_op = op_s
                            .and_then(|v| v.trim().parse::<f64>().ok())
                            .unwrap_or(1.0)
                            .clamp(0.0, 1.0);
                        match parse_color(col_s, [0, 0, 0, 255]) {
                            Ok(mut c) => {
                                c[3] = ((c[3] as f64) * stop_op).round().clamp(0.0, 255.0) as u8;
                                stops.push(GradientStop { offset, color: c });
                            }
                            Err(e) => errors.push(format!("line {}: {e}", ch.line)),
                        }
                    }
                }
                stops.sort_by(|a, b| a.offset.total_cmp(&b.offset));
                let href = get_href(node).and_then(|h| h.trim().strip_prefix('#')).map(|s| s.to_string());
                raw_grads.insert(
                    id.to_string(),
                    RawGradient {
                        is_radial,
                        href,
                        x1: node.attr("x1").and_then(parse_percent_or_num),
                        y1: node.attr("y1").and_then(parse_percent_or_num),
                        x2: node.attr("x2").and_then(parse_percent_or_num),
                        y2: node.attr("y2").and_then(parse_percent_or_num),
                        cx: node.attr("cx").and_then(parse_percent_or_num),
                        cy: node.attr("cy").and_then(parse_percent_or_num),
                        r: node.attr("r").and_then(parse_percent_or_num),
                        fx: node.attr("fx").and_then(parse_percent_or_num),
                        fy: node.attr("fy").and_then(parse_percent_or_num),
                        units,
                        spread,
                        transform,
                        stops,
                    },
                );
            }
            return;
        }
        "clipPath" => {
            if let Some(id) = node.attr("id") {
                let units = match node.attr("clipPathUnits").unwrap_or("userSpaceOnUse").trim() {
                    "objectBoundingBox" => CoordUnits::ObjectBoundingBox,
                    _ => CoordUnits::UserSpaceOnUse,
                };
                let mut shapes = Vec::new();
                collect_clip_shapes(node, Affine::IDENTITY, tol, &mut shapes, errors);
                clip_paths.insert(id.to_string(), ClipPathDef { units, shapes });
            }
            return;
        }
        "filter" => {
            if let Some(id) = node.attr("id") {
                let mut blur: Option<(f64, f64)> = None;
                let mut merge_source = false;
                let mut drop_shadow: Option<FilterKind> = None;
                for ch in &node.children {
                    match strip_ns(&ch.tag) {
                        "feGaussianBlur" => {
                            let sd = ch.attr("stdDeviation").unwrap_or("2");
                            if let Ok(nums) = parse_number_list(sd) {
                                let sx = nums.first().copied().unwrap_or(2.0).max(0.0);
                                let sy = nums.get(1).copied().unwrap_or(sx).max(0.0);
                                blur = Some((sx, sy));
                            }
                        }
                        "feMerge" => {
                            for m in &ch.children {
                                if strip_ns(&m.tag) == "feMergeNode"
                                    && m.attr("in") == Some("SourceGraphic")
                                {
                                    merge_source = true;
                                }
                            }
                        }
                        "feDropShadow" => {
                            let dx = ch.attr("dx").and_then(parse_length).unwrap_or(0.0);
                            let dy = ch.attr("dy").and_then(parse_length).unwrap_or(2.0);
                            let sd = ch.attr("stdDeviation").unwrap_or("2");
                            let nums = parse_number_list(sd).unwrap_or_else(|_| vec![2.0]);
                            let sx = nums.first().copied().unwrap_or(2.0).max(0.0);
                            let sy = nums.get(1).copied().unwrap_or(sx).max(0.0);
                            let mut col = parse_color(
                                ch.attr("flood-color").unwrap_or("#000000"),
                                [0, 0, 0, 255],
                            )
                            .unwrap_or([0, 0, 0, 255]);
                            let fo = ch
                                .attr("flood-opacity")
                                .and_then(|v| v.trim().parse::<f64>().ok())
                                .unwrap_or(0.6)
                                .clamp(0.0, 1.0);
                            col[3] = ((col[3] as f64) * fo).round().clamp(0.0, 255.0) as u8;
                            drop_shadow = Some(FilterKind::DropShadow {
                                dx,
                                dy,
                                std_x: sx,
                                std_y: sy,
                                color: col,
                            });
                        }
                        other => {
                            warnings.push(format!(
                                "line {}: unsupported filter primitive <{other}> ignored",
                                ch.line
                            ));
                        }
                    }
                }
                if let Some(ds) = drop_shadow {
                    filters.insert(id.to_string(), ds);
                } else if let Some((std_x, std_y)) = blur {
                    filters.insert(
                        id.to_string(),
                        FilterKind::GaussianBlur { std_x, std_y, merge_source },
                    );
                }
            }
            return;
        }
        _ => {}
    }

    for ch in &node.children {
        collect_defs(
            ch,
            tol,
            nodes_by_id,
            raw_grads,
            clip_paths,
            filters,
            warnings,
            errors,
        );
    }
}

fn collect_clip_shapes(
    node: &XmlNode,
    parent_tf: Affine,
    tol: f64,
    out: &mut Vec<(PathData, FillRule)>,
    errors: &mut Vec<String>,
) {
    for ch in &node.children {
        let local_tf = match ch.attr("transform") {
            Some(ts) => parse_transform(ts).unwrap_or(Affine::IDENTITY),
            None => Affine::IDENTITY,
        };
        let tf = parent_tf.mul(local_tf);
        let tag = strip_ns(&ch.tag);
        if tag == "g" {
            collect_clip_shapes(ch, tf, tol, out, errors);
        } else if let Some(path) = parse_shape_geometry(ch, tol, errors) {
            let rule = match ch.attr("clip-rule").or_else(|| ch.attr("fill-rule")) {
                Some("evenodd") => FillRule::EvenOdd,
                _ => FillRule::NonZero,
            };
            out.push((path.transform(tf), rule));
        }
    }
}

fn resolve_gradient(
    id: &str,
    raw: &HashMap<String, RawGradient>,
    depth: u32,
) -> Option<Gradient> {
    if depth > 8 {
        return None;
    }
    let g = raw.get(id)?;
    let parent = g
        .href
        .as_deref()
        .and_then(|pid| resolve_gradient(pid, raw, depth + 1));

    let stops = if !g.stops.is_empty() {
        g.stops.clone()
    } else if let Some(ref p) = parent {
        p.stops.clone()
    } else {
        Vec::new()
    };

    let units = g
        .units
        .or_else(|| parent.as_ref().map(|p| p.units))
        .unwrap_or(CoordUnits::ObjectBoundingBox);
    let spread = g
        .spread
        .or_else(|| parent.as_ref().map(|p| p.spread))
        .unwrap_or(SpreadMethod::Pad);
    let transform = g
        .transform
        .or_else(|| parent.as_ref().map(|p| p.transform))
        .unwrap_or(Affine::IDENTITY);

    let kind = if g.is_radial {
        let (pcx, pcy, pr, pfx, pfy) = match parent.as_ref().map(|p| &p.kind) {
            Some(GradientKind::Radial { cx, cy, r, fx, fy }) => (*cx, *cy, *r, *fx, *fy),
            _ => (0.5, 0.5, 0.5, 0.5, 0.5),
        };
        let cx = g.cx.unwrap_or(pcx);
        let cy = g.cy.unwrap_or(pcy);
        let r = g.r.unwrap_or(pr);
        let fx = g.fx.unwrap_or(if g.cx.is_some() { cx } else { pfx });
        let fy = g.fy.unwrap_or(if g.cy.is_some() { cy } else { pfy });
        GradientKind::Radial { cx, cy, r, fx, fy }
    } else {
        let (px1, py1, px2, py2) = match parent.as_ref().map(|p| &p.kind) {
            Some(GradientKind::Linear { x1, y1, x2, y2 }) => (*x1, *y1, *x2, *y2),
            _ => (0.0, 0.0, 1.0, 0.0),
        };
        GradientKind::Linear {
            x1: g.x1.unwrap_or(px1),
            y1: g.y1.unwrap_or(py1),
            x2: g.x2.unwrap_or(px2),
            y2: g.y2.unwrap_or(py2),
        }
    };

    Some(Gradient { kind, units, spread, transform, stops })
}

struct BuildCtx<'a> {
    tol: f64,
    nodes_by_id: &'a HashMap<String, &'a XmlNode>,
    gradients: &'a HashMap<String, Gradient>,
    clip_paths: &'a HashMap<String, ClipPathDef>,
    filters: &'a HashMap<String, FilterKind>,
    stats: &'a mut SvgStats,
    warnings: &'a mut Vec<String>,
    errors: &'a mut Vec<String>,
}

impl<'a> BuildCtx<'a> {
    fn visit_node(
        &mut self,
        node: &XmlNode,
        parent_style: &InheritedStyle,
        parent_tf: Affine,
        depth: u32,
        out: &mut Vec<RenderItem>,
    ) {
        if depth > 32 {
            self.errors.push(format!("line {}: maximum SVG nesting/use depth exceeded", node.line));
            return;
        }
        let tag = strip_ns(&node.tag);
        self.stats.elements += 1;

        // Skip non-rendering metadata/definition subtrees
        if matches!(
            tag,
            "defs" | "title" | "desc" | "metadata" | "linearGradient" | "radialGradient" | "clipPath" | "filter" | "style" | "symbol"
        ) {
            return;
        }
        if tag == "script" {
            self.errors.push(format!("line {}: <script> is disallowed in game UI SVGs", node.line));
            return;
        }
        if tag == "image" {
            if let Some(h) = get_href(node) {
                if h.starts_with("http://") || h.starts_with("https://") {
                    self.errors.push(format!("line {}: external <image> URL '{h}' is disallowed", node.line));
                    return;
                }
            }
            self.warnings.push(format!("line {}: <image> element skipped (vector-only renderer)", node.line));
            return;
        }
        if tag == "text" || tag == "tspan" {
            self.warnings.push(format!(
                "line {}: <{tag}> skipped (render text in-engine for localization & crisp DPI scaling)",
                node.line
            ));
            return;
        }

        let style_map = parse_inline_style(node.attr("style").unwrap_or(""));
        let display = style_map
            .get("display")
            .map(|s| s.as_str())
            .or_else(|| node.attr("display"))
            .unwrap_or("");
        if display.eq_ignore_ascii_case("none") {
            return;
        }

        let style = resolve_style(node, parent_style, self.errors);
        let opacity = style_map
            .get("opacity")
            .map(|s| s.as_str())
            .or_else(|| node.attr("opacity"))
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);

        let local_tf = match node.attr("transform") {
            Some(ts) => match parse_transform(ts) {
                Ok(m) => m,
                Err(e) => {
                    self.errors.push(format!("line {}: {e}", node.line));
                    Affine::IDENTITY
                }
            },
            None => Affine::IDENTITY,
        };
        let tf = parent_tf.mul(local_tf);

        let clip_path = style_map
            .get("clip-path")
            .map(|s| s.as_str())
            .or_else(|| node.attr("clip-path"))
            .and_then(|v| {
                if v.trim() == "none" {
                    None
                } else {
                    match parse_url_ref(v) {
                        Some(id) => {
                            if !self.clip_paths.contains_key(&id) {
                                self.errors.push(format!("line {}: unknown clip-path '#{id}'", node.line));
                                None
                            } else {
                                Some(id)
                            }
                        }
                        None => None,
                    }
                }
            });

        let filter = style_map
            .get("filter")
            .map(|s| s.as_str())
            .or_else(|| node.attr("filter"))
            .and_then(|v| {
                if v.trim() == "none" {
                    None
                } else {
                    match parse_url_ref(v) {
                        Some(id) => {
                            if !self.filters.contains_key(&id) {
                                self.errors.push(format!("line {}: unknown filter '#{id}'", node.line));
                                None
                            } else {
                                Some(id)
                            }
                        }
                        None => None,
                    }
                }
            });

        match tag {
            "g" | "svg" => {
                let needs_group = opacity < 0.999 || clip_path.is_some() || filter.is_some();
                if needs_group {
                    let mut child_items = Vec::new();
                    for ch in &node.children {
                        self.visit_node(ch, &style, tf, depth + 1, &mut child_items);
                    }
                    if !child_items.is_empty() {
                        out.push(RenderItem::Group(DrawGroup {
                            items: child_items,
                            opacity,
                            clip_path,
                            filter,
                            transform: tf,
                        }));
                    }
                } else {
                    for ch in &node.children {
                        self.visit_node(ch, &style, tf, depth + 1, out);
                    }
                }
            }
            "use" => {
                let href = match get_href(node).and_then(|h| h.trim().strip_prefix('#')) {
                    Some(id) => id,
                    None => {
                        self.errors.push(format!("line {}: <use> missing #id href", node.line));
                        return;
                    }
                };
                let target = match self.nodes_by_id.get(href) {
                    Some(&t) => t,
                    None => {
                        self.errors.push(format!("line {}: <use> target '#{href}' not found", node.line));
                        return;
                    }
                };
                let ux = node.attr("x").and_then(parse_length).unwrap_or(0.0);
                let uy = node.attr("y").and_then(parse_length).unwrap_or(0.0);
                let use_tf = tf.mul(Affine::translate(ux, uy));
                let needs_group = opacity < 0.999 || clip_path.is_some() || filter.is_some();
                if needs_group {
                    let mut child_items = Vec::new();
                    self.visit_node(target, &style, use_tf, depth + 1, &mut child_items);
                    if !child_items.is_empty() {
                        out.push(RenderItem::Group(DrawGroup {
                            items: child_items,
                            opacity,
                            clip_path,
                            filter,
                            transform: use_tf,
                        }));
                    }
                } else {
                    self.visit_node(target, &style, use_tf, depth + 1, out);
                }
            }
            "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" | "path" => {
                if !style.visible || opacity <= 0.0 {
                    return;
                }
                if let Paint::Gradient(ref gid) = style.fill {
                    if !self.gradients.contains_key(gid) {
                        self.errors.push(format!("line {}: unknown fill gradient '#{gid}'", node.line));
                    }
                }
                if let Paint::Gradient(ref gid) = style.stroke {
                    if !self.gradients.contains_key(gid) {
                        self.errors.push(format!("line {}: unknown stroke gradient '#{gid}'", node.line));
                    }
                }
                if let Some(path) = parse_shape_geometry(node, self.tol, self.errors) {
                    if !path.subpaths.is_empty() {
                        let mut bb = path.transform(tf).bbox();
                        if !matches!(style.stroke, Paint::None) {
                            bb.expand(style.stroke_style.width * 0.5 * tf.avg_scale());
                        }
                        // Only expand overall content bbox when unclipped (clipped content stays inside clip)
                        if clip_path.is_none() {
                            self.stats.content_bbox.union(bb);
                        } else if let Some(ref cid) = clip_path {
                            if let Some(cdef) = self.clip_paths.get(cid) {
                                for (cp, _) in &cdef.shapes {
                                    self.stats.content_bbox.union(cp.bbox());
                                }
                            }
                        }
                        self.stats.shapes += 1;
                        out.push(RenderItem::Shape(DrawShape {
                            path,
                            transform: tf,
                            fill: style.fill,
                            fill_opacity: style.fill_opacity,
                            fill_rule: style.fill_rule,
                            stroke: style.stroke,
                            stroke_opacity: style.stroke_opacity,
                            stroke_style: style.stroke_style,
                            opacity,
                            clip_path,
                            filter,
                        }));
                    }
                }
            }
            other => {
                self.warnings.push(format!("line {}: unsupported SVG element <{other}> skipped", node.line));
            }
        }
    }
}

fn parse_shape_geometry(node: &XmlNode, tol: f64, errors: &mut Vec<String>) -> Option<PathData> {
    let tag = strip_ns(&node.tag);
    match tag {
        "rect" => {
            let x = node.attr("x").and_then(parse_length).unwrap_or(0.0);
            let y = node.attr("y").and_then(parse_length).unwrap_or(0.0);
            let w = node.attr("width").and_then(parse_length).unwrap_or(0.0);
            let h = node.attr("height").and_then(parse_length).unwrap_or(0.0);
            let rx_opt = node.attr("rx").and_then(parse_length);
            let ry_opt = node.attr("ry").and_then(parse_length);
            let rx = rx_opt.or(ry_opt).unwrap_or(0.0);
            let ry = ry_opt.or(rx_opt).unwrap_or(0.0);
            Some(rect_path(x, y, w, h, rx, ry, tol))
        }
        "circle" => {
            let cx = node.attr("cx").and_then(parse_length).unwrap_or(0.0);
            let cy = node.attr("cy").and_then(parse_length).unwrap_or(0.0);
            let r = node.attr("r").and_then(parse_length).unwrap_or(0.0);
            Some(ellipse_path(cx, cy, r, r, tol))
        }
        "ellipse" => {
            let cx = node.attr("cx").and_then(parse_length).unwrap_or(0.0);
            let cy = node.attr("cy").and_then(parse_length).unwrap_or(0.0);
            let rx = node.attr("rx").and_then(parse_length).unwrap_or(0.0);
            let ry = node.attr("ry").and_then(parse_length).unwrap_or(0.0);
            Some(ellipse_path(cx, cy, rx, ry, tol))
        }
        "line" => {
            let x1 = node.attr("x1").and_then(parse_length).unwrap_or(0.0);
            let y1 = node.attr("y1").and_then(parse_length).unwrap_or(0.0);
            let x2 = node.attr("x2").and_then(parse_length).unwrap_or(0.0);
            let y2 = node.attr("y2").and_then(parse_length).unwrap_or(0.0);
            Some(PathData {
                subpaths: vec![Subpath {
                    pts: vec![Pt::new(x1, y1), Pt::new(x2, y2)],
                    closed: false,
                }],
            })
        }
        "polyline" | "polygon" => {
            let raw = node.attr("points").unwrap_or("");
            match parse_number_list(raw) {
                Ok(nums) => {
                    if nums.len() < 4 || nums.len() % 2 != 0 {
                        errors.push(format!(
                            "line {}: <{tag}> points must contain an even list of >=4 coordinates",
                            node.line
                        ));
                        return None;
                    }
                    let pts: Vec<Pt> = nums.chunks_exact(2).map(|c| Pt::new(c[0], c[1])).collect();
                    Some(PathData {
                        subpaths: vec![Subpath { pts, closed: tag == "polygon" }],
                    })
                }
                Err(e) => {
                    errors.push(format!("line {}: invalid <{tag}> points: {e}", node.line));
                    None
                }
            }
        }
        "path" => {
            let d = node.attr("d").unwrap_or("");
            match parse_svg_path(d, tol) {
                Ok(p) => Some(p),
                Err(e) => {
                    errors.push(format!("line {}: invalid <path d>: {e}", node.line));
                    None
                }
            }
        }
        _ => None,
    }
}

fn resolve_style(
    node: &XmlNode,
    parent: &InheritedStyle,
    errors: &mut Vec<String>,
) -> InheritedStyle {
    let mut s = parent.clone();
    let inline = parse_inline_style(node.attr("style").unwrap_or(""));
    let get = |k: &str| -> Option<&str> {
        inline.get(k).map(|v| v.as_str()).or_else(|| node.attr(k))
    };

    if let Some(c) = get("color") {
        if let Ok(col) = parse_color(c, s.color) {
            s.color = col;
        }
    }
    if let Some(v) = get("visibility") {
        s.visible = !matches!(v.trim(), "hidden" | "collapse");
    }
    if let Some(v) = get("fill") {
        match parse_paint(v, s.color) {
            Ok(p) => s.fill = p,
            Err(e) => errors.push(format!("line {}: {e}", node.line)),
        }
    }
    if let Some(v) = get("fill-opacity") {
        if let Ok(op) = v.trim().parse::<f64>() {
            s.fill_opacity = op.clamp(0.0, 1.0);
        }
    }
    if let Some(v) = get("fill-rule") {
        s.fill_rule = match v.trim() {
            "evenodd" => FillRule::EvenOdd,
            _ => FillRule::NonZero,
        };
    }
    if let Some(v) = get("stroke") {
        match parse_paint(v, s.color) {
            Ok(p) => s.stroke = p,
            Err(e) => errors.push(format!("line {}: {e}", node.line)),
        }
    }
    if let Some(v) = get("stroke-opacity") {
        if let Ok(op) = v.trim().parse::<f64>() {
            s.stroke_opacity = op.clamp(0.0, 1.0);
        }
    }
    if let Some(v) = get("stroke-width") {
        if let Some(w) = parse_length(v) {
            s.stroke_style.width = w.max(0.0);
        }
    }
    if let Some(v) = get("stroke-linecap") {
        s.stroke_style.cap = match v.trim() {
            "round" => LineCap::Round,
            "square" => LineCap::Square,
            _ => LineCap::Butt,
        };
    }
    if let Some(v) = get("stroke-linejoin") {
        s.stroke_style.join = match v.trim() {
            "round" => LineJoin::Round,
            "bevel" => LineJoin::Bevel,
            _ => LineJoin::Miter,
        };
    }
    if let Some(v) = get("stroke-miterlimit") {
        if let Ok(ml) = v.trim().parse::<f64>() {
            s.stroke_style.miter_limit = ml.max(1.0);
        }
    }
    if let Some(v) = get("stroke-dasharray") {
        let t = v.trim();
        if t.eq_ignore_ascii_case("none") || t.is_empty() {
            s.stroke_style.dasharray.clear();
        } else if let Ok(nums) = parse_number_list(t) {
            s.stroke_style.dasharray = nums;
        }
    }
    if let Some(v) = get("stroke-dashoffset") {
        if let Some(off) = parse_length(v) {
            s.stroke_style.dashoffset = off;
        }
    }

    s
}

fn parse_inline_style(s: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for decl in s.split(';') {
        if let Some((k, v)) = decl.split_once(':') {
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim().to_string();
            if !k.is_empty() && !v.is_empty() {
                map.insert(k, v);
            }
        }
    }
    map
}

fn parse_length(s: &str) -> Option<f64> {
    let t = s.trim();
    let t = t.strip_suffix("px").unwrap_or(t).trim();
    t.parse::<f64>().ok()
}

fn parse_percent_or_num(s: &str) -> Option<f64> {
    let t = s.trim();
    if let Some(p) = t.strip_suffix('%') {
        p.trim().parse::<f64>().ok().map(|v| v * 0.01)
    } else {
        parse_length(t)
    }
}

fn parse_paint(s: &str, current_color: [u8; 4]) -> Result<Paint, String> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("none") {
        return Ok(Paint::None);
    }
    if let Some(id) = parse_url_ref(t) {
        return Ok(Paint::Gradient(id));
    }
    Ok(Paint::Color(parse_color(t, current_color)?))
}

pub fn parse_color(s: &str, current_color: [u8; 4]) -> Result<[u8; 4], String> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("transparent") {
        return Ok([0, 0, 0, 0]);
    }
    if t.eq_ignore_ascii_case("currentColor") {
        return Ok(current_color);
    }
    if let Some(hex) = t.strip_prefix('#') {
        if !hex.is_ascii() {
            return Err(format!("invalid hex color '{t}'"));
        }
        return match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).map_err(|_| format!("invalid hex '{t}'"))?;
                let g = u8::from_str_radix(&hex[1..2], 16).map_err(|_| format!("invalid hex '{t}'"))?;
                let b = u8::from_str_radix(&hex[2..3], 16).map_err(|_| format!("invalid hex '{t}'"))?;
                Ok([r * 17, g * 17, b * 17, 255])
            }
            4 => {
                let r = u8::from_str_radix(&hex[0..1], 16).map_err(|_| format!("invalid hex '{t}'"))?;
                let g = u8::from_str_radix(&hex[1..2], 16).map_err(|_| format!("invalid hex '{t}'"))?;
                let b = u8::from_str_radix(&hex[2..3], 16).map_err(|_| format!("invalid hex '{t}'"))?;
                let a = u8::from_str_radix(&hex[3..4], 16).map_err(|_| format!("invalid hex '{t}'"))?;
                Ok([r * 17, g * 17, b * 17, a * 17])
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).map_err(|_| format!("invalid hex '{t}'"))?;
                let g = u8::from_str_radix(&hex[2..4], 16).map_err(|_| format!("invalid hex '{t}'"))?;
                let b = u8::from_str_radix(&hex[4..6], 16).map_err(|_| format!("invalid hex '{t}'"))?;
                Ok([r, g, b, 255])
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).map_err(|_| format!("invalid hex '{t}'"))?;
                let g = u8::from_str_radix(&hex[2..4], 16).map_err(|_| format!("invalid hex '{t}'"))?;
                let b = u8::from_str_radix(&hex[4..6], 16).map_err(|_| format!("invalid hex '{t}'"))?;
                let a = u8::from_str_radix(&hex[6..8], 16).map_err(|_| format!("invalid hex '{t}'"))?;
                Ok([r, g, b, a])
            }
            _ => Err(format!("invalid hex color '{t}'")),
        };
    }

    if let Some(inner) = t
        .strip_prefix("rgba(")
        .or_else(|| t.strip_prefix("rgb("))
        .and_then(|r| r.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
        if parts.len() == 3 || parts.len() == 4 {
            let ch = |s: &str| -> Result<u8, String> {
                if let Some(p) = s.strip_suffix('%') {
                    let v: f64 = p.trim().parse().map_err(|_| format!("bad rgb '{t}'"))?;
                    Ok((v * 2.55).round().clamp(0.0, 255.0) as u8)
                } else {
                    let v: f64 = s.parse().map_err(|_| format!("bad rgb '{t}'"))?;
                    Ok(v.round().clamp(0.0, 255.0) as u8)
                }
            };
            let r = ch(parts[0])?;
            let g = ch(parts[1])?;
            let b = ch(parts[2])?;
            let a = if parts.len() == 4 {
                let av: f64 = parts[3].parse().map_err(|_| format!("bad alpha '{t}'"))?;
                (av * 255.0).round().clamp(0.0, 255.0) as u8
            } else {
                255
            };
            return Ok([r, g, b, a]);
        }
    }

    match t.to_ascii_lowercase().as_str() {
        "black" => Ok([0, 0, 0, 255]),
        "white" => Ok([255, 255, 255, 255]),
        "red" => Ok([255, 0, 0, 255]),
        "green" => Ok([0, 128, 0, 255]),
        "lime" => Ok([0, 255, 0, 255]),
        "blue" => Ok([0, 0, 255, 255]),
        "yellow" => Ok([255, 255, 0, 255]),
        "cyan" | "aqua" => Ok([0, 255, 255, 255]),
        "magenta" | "fuchsia" => Ok([255, 0, 255, 255]),
        "orange" => Ok([255, 165, 0, 255]),
        "gold" => Ok([255, 215, 0, 255]),
        "purple" => Ok([128, 0, 128, 255]),
        "indigo" => Ok([75, 0, 130, 255]),
        "violet" => Ok([238, 130, 238, 255]),
        "crimson" => Ok([220, 20, 60, 255]),
        "coral" => Ok([255, 127, 80, 255]),
        "teal" => Ok([0, 128, 128, 255]),
        "navy" => Ok([0, 0, 128, 255]),
        "maroon" => Ok([128, 0, 0, 255]),
        "olive" => Ok([128, 128, 0, 255]),
        "silver" => Ok([192, 192, 192, 255]),
        "gray" | "grey" => Ok([128, 128, 128, 255]),
        "darkgray" | "darkgrey" => Ok([169, 169, 169, 255]),
        "lightgray" | "lightgrey" => Ok([211, 211, 211, 255]),
        "ivory" => Ok([255, 255, 240, 255]),
        "brown" => Ok([165, 42, 42, 255]),
        "pink" => Ok([255, 192, 203, 255]),
        _ => Err(format!("unsupported color '{t}'")),
    }
}
