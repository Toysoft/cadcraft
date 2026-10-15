//! What Craft AI does inside CADCraft. Craft AI is one assistant across the ArtCraft family
//! — the model, the runtime and the shared folder belong to the family — but its *work* is the
//! app's own: the system prompt tells the model which app it is inside and what that app does,
//! and every ready-made task is framed for that app's world, so the answer that comes back is
//! about layers, blocks and dimension styles, not generic advice.
//!
//! Each app of the family carries its own `tasks.rs` — VectorCraft's asks about paths and
//! Pathfinder steps, PhotoCraft's about layers and masks, and so on — same model, same shared
//! folder, different craft. The labels and hints are catalog keys: the palette shows them
//! translated.

/// The app Craft AI is embedded in, as the shared folder's records name it.
pub const APP: &str = "cadcraft";

/// One ready-made task of the Assistant tab.
pub struct Task {
    /// The label the picker shows (a catalog key).
    pub label: &'static str,
    /// The input field's hint (a catalog key).
    pub hint: &'static str,
}

/// The tasks, in the picker's order.
pub const TASKS: &[Task] = &[
    Task { label: "Ask anything", hint: "Ask in any language" },
    Task { label: "Explain a command or entity", hint: "The command, entity or palette to explain" },
    Task { label: "Dimensioning advice", hint: "What is being dimensioned, and the drawing's scale" },
    Task { label: "Layer organisation", hint: "The layers there are, and what the drawing shows" },
    Task { label: "DXF and DWG questions", hint: "The file, its version, or what round-trips" },
    Task { label: "How do I…", hint: "What you want to achieve" },
];

/// Tell the model where it is and how to answer: briefly, practically, in the language the user
/// wrote in, about this app's own world — the words of its interface, the files it opens and
/// writes. The last line is the honesty clause: a local model can be wrong, and it is told to
/// say so rather than invent.
pub fn system_prompt() -> String {
    "You are Craft AI, the assistant built into CADCraft, a desktop 2D CAD editor in the \
     AutoCAD tradition. CADCraft's world, in its own words: the drawing entities — lines, \
     polylines, circles, arcs, splines, ellipses, points, donuts, regions and hatches; layers, \
     their colours, linetypes and lineweights; blocks, block attributes and external \
     references; dimensions — linear, aligned, radius, diameter and angular — dimension \
     styles and leaders; the Drafting Settings of snap and grid, and the object snaps; model \
     space and paper space, layouts and viewports; the command line with its aliases and \
     AutoLISP; DXF opened and written, DWG too, and SVG and PDF; and the palettes — Tool Sets, \
     Layers, Properties, QuickCalc and External References. Answer in the language the user \
     wrote in, Arabic included. Be brief and practical: name the command or the palette, give \
     the steps as a short list, say what to watch out for, and when a question is not about \
     CADCraft or CAD, answer it anyway, briefly. Never invent menu entries, command names or \
     palette names: if you are not sure CADCraft has a feature, say so plainly."
        .to_string()
}

/// Frame the user's input for the task they picked.
pub fn user_prompt(task: usize, input: &str) -> String {
    let input = input.trim();
    match TASKS[task.min(TASKS.len() - 1)].label {
        "Explain a command or entity" => format!(
            "In CADCraft, explain how this command, entity or palette works, and when to reach for it: {input}"
        ),
        "Dimensioning advice" => format!(
            "Advise on dimensioning this in CADCraft — the dimension type, its style and the placement rules: {input}"
        ),
        "Layer organisation" => format!(
            "Design the layer structure for this CADCraft drawing — names, colours, linetypes, and what belongs on each: {input}"
        ),
        "DXF and DWG questions" => format!(
            "Answer this DXF or DWG question for CADCraft — versions, entities, what round-trips and what to check: {input}"
        ),
        "How do I…" => format!("In CADCraft, how do I: {input}"),
        _ => input.to_string(),
    }
}
