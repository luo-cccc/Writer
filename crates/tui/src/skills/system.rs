//! System-skill installer: bundles core skills and auto-installs them on first launch.

use std::fs;
use std::path::Path;

const BUNDLED_SKILL_VERSION: &str = "4";
const BUNDLED_SYSTEM_SKILLS: &[(&str, &str)] = &[
    (
        "skill-creator",
        include_str!("../../assets/skills/skill-creator/SKILL.md"),
    ),
    ("human-texture", HUMAN_TEXTURE_SKILL),
    ("anti-ai-prose", ANTI_AI_PROSE_SKILL),
    ("dialogue", DIALOGUE_SKILL),
    ("suspense", SUSPENSE_SKILL),
    ("character-arc", CHARACTER_ARC_SKILL),
    ("webnovel-pacing", WEBNOVEL_PACING_SKILL),
    ("scene-pressure", SCENE_PRESSURE_SKILL),
    ("worldbuilding", WORLDBUILDING_SKILL),
    ("xianxia-craft", XIANXIA_CRAFT_SKILL),
];

/// Install bundled system skills into `skills_dir`.
///
/// Behaviour:
/// - Fresh install (no marker, no dirs): installs bundled `SKILL.md` files and writes
///   the version marker.
/// - Version bump (marker present with older version, dir present): installs missing
///   bundled skills and updates the marker, without overwriting edited `SKILL.md` files.
/// - User deleted the dir while marker still present at same version: leaves it gone.
/// - Idempotent: calling twice with no changes is a no-op.
///
/// Errors are I/O errors from the filesystem; the caller should log them but not
/// abort startup.
pub fn install_system_skills(skills_dir: &Path) -> std::io::Result<()> {
    let marker = skills_dir.join(".system-installed-version");
    let installed_version = fs::read_to_string(&marker)
        .ok()
        .map(|s| s.trim().to_string());
    let any_dir_exists = BUNDLED_SYSTEM_SKILLS
        .iter()
        .any(|(name, _)| skills_dir.join(name).exists());

    // Re-install only when BOTH conditions hold:
    //   (a) bundled version is newer than what is recorded in the marker, AND
    //   (b) the skill directory still exists (user hasn't intentionally deleted it).
    // Fresh install (no marker AND no dir) is also handled.
    let should_install = match (installed_version.as_deref(), any_dir_exists) {
        (None, false) => true,
        (Some(v), true) if v != BUNDLED_SKILL_VERSION => true,
        _ => false,
    };

    if should_install {
        fs::create_dir_all(skills_dir)?;
        for (name, body) in BUNDLED_SYSTEM_SKILLS {
            let target_dir = skills_dir.join(name);
            let target_file = target_dir.join("SKILL.md");
            if !target_file.exists() {
                fs::create_dir_all(&target_dir)?;
                fs::write(target_file, body)?;
            }
        }
        fs::write(&marker, BUNDLED_SKILL_VERSION)?;
    }
    Ok(())
}

/// Remove the `skill-creator` system skill and its version marker.
///
/// Intended for tests and `deepseek setup --clean`.  Ignores missing files.
#[allow(dead_code)]
pub fn uninstall_system_skills(skills_dir: &Path) -> std::io::Result<()> {
    let marker = skills_dir.join(".system-installed-version");
    for (name, _) in BUNDLED_SYSTEM_SKILLS {
        let target_dir = skills_dir.join(name);
        if target_dir.exists() {
            fs::remove_dir_all(&target_dir)?;
        }
    }
    if marker.exists() {
        fs::remove_file(&marker)?;
    }
    Ok(())
}

const HUMAN_TEXTURE_SKILL: &str = r#"---
name: human-texture
description: Add grounded human texture to long-form Chinese fiction without changing canon.
---

Use this skill when drafting, revising, or diagnosing prose that needs more lived-in human texture.

Rules:
- Treat `book.toml`, `bible/`, `cards/`, `outline/`, chapters, and `memory/` as canon.
- Do not overwrite the user's plot or force a template.
- Prefer concrete pressure: a choice, a delay, a bodily cost, a public consequence, a damaged object, a changed relationship.
- Externalize emotion through action, interruption, silence, object handling, posture, misdirection, and what a character refuses to say.
- Preserve uneven but natural Chinese rhythm; avoid making every paragraph the same shape.
- Output should be usable in `brief.md`, `craft_plan.md`, `audit.md`, or chapter prose depending on the user's request.
"#;

const ANTI_AI_PROSE_SKILL: &str = r#"---
name: anti-ai-prose
description: Diagnose and reduce generic AI-shaped prose while preserving author intent.
---

Use this skill for prose cleanup and audit tasks.

Rules:
- Do not score the chapter or impose a universal structure.
- Flag generic transitions, empty atmosphere, abstract emotional naming, repeated paragraph cadence, exposition disguised as dialogue, and summary endings.
- Replace only with story-specific pressure, concrete action, sharper dialogue purpose, or a consequence already supported by canon.
- Keep changes conservative unless the user explicitly asks for a rewrite.
- If writing an audit, separate BLOCKER/MAJOR/MINOR from optional texture notes.
"#;

const DIALOGUE_SKILL: &str = r#"---
name: dialogue
description: Improve dialogue, subtext, and character voice in novel scenes.
---

Use this skill when a scene depends on conversation, confrontation, negotiation, concealment, or emotional avoidance.

Rules:
- Respect canon from `book.toml`, `bible/`, `cards/`, chapters, and `memory/`; dialogue may reveal or conceal facts, but must not invent unsupported facts.
- Every important line should try to get, hide, test, threaten, bargain, confess, redirect, or wound.
- Characters should not share one voice; preserve class, age, intimacy, power, education, and current fear.
- Exposition belongs inside conflict, misunderstanding, leverage, payment, or consequence.
- Track who knows what before and after the exchange.
- End dialogue with a changed relation, changed information state, or changed choice.
- Do not force every conversation into the same interrogation, confession, banter, or exposition template.
"#;

const SUSPENSE_SKILL: &str = r#"---
name: suspense
description: Manage suspense, promises, foreshadowing, reveals, and payoff timing.
---

Use this skill for mystery, thriller, cultivation secrets, political schemes, and long-form promise tracking.

Rules:
- Distinguish new promise, active pressure, misdirection, reveal, payoff, delayed payoff, and abandoned thread.
- Do not reveal secrets just because they are in the context; respect character knowledge boundaries.
- A payoff should cost something or change a relationship, resource, risk, or future option.
- Record durable promises and payoff status as candidates for `memory/foreshadowing.jsonl` or `memory/candidates/*.json`.
- Avoid ending every chapter with the same hollow question or template cliffhanger; pair open loops with concrete consequences.
"#;

const CHARACTER_ARC_SKILL: &str = r#"---
name: character-arc
description: Track character desire, fear, belief, state changes, and relationship movement.
---

Use this skill for planning, brief generation, audits, and revision of character-driven chapters.

Rules:
- Separate outer want, inner need, fear, secret, self-deception, current leverage, and current wound.
- Track what the character knows, does not know, pretends not to know, and refuses to admit.
- A chapter-level arc can be small: a new tactic, a colder relationship, a public mask cracking, a private belief reinforced.
- Do not make growth linear, universally healthy, or formulaic; regression, avoidance, and costly compromise are valid.
- Durable state changes should become memory candidates with evidence.
"#;

const WEBNOVEL_PACING_SKILL: &str = r#"---
name: webnovel-pacing
description: Shape serial-fiction momentum without flattening every chapter into a formula.
---

Use this skill for webnovel, serial, and high-retention chapter planning, drafting, audit, or revision.

Rules:
- Treat pacing as pressure over time, not a required beat sheet.
- Track what the reader is waiting for: payoff, escalation, reveal, competence display, emotional confrontation, resource gain/loss, or public reversal.
- Keep each chapter's promise specific; avoid generic "hook" endings that do not change risk, information, relationship, or options.
- Vary chapter motion: compression, delay, misdirection, consequence, aftermath, setup, confrontation, and release all have uses.
- Do not sacrifice canon, character knowledge boundaries, or prose texture for empty speed.
- For audits, name the exact stalled pressure and suggest one compatible acceleration or delay.
"#;

const SCENE_PRESSURE_SKILL: &str = r#"---
name: scene-pressure
description: Build scenes around concrete pressure, choices, reversals, and consequences.
---

Use this skill when a chapter scene feels static, expository, or emotionally vague.

Rules:
- Identify the scene's immediate pressure: time, secrecy, body, money, public status, danger, desire, debt, witness, resource, or relationship.
- Give each principal character a tactic, a line they will not cross yet, and something they can lose before the scene ends.
- Replace explanation with action under pressure: interruption, bargaining, concealment, testing, refusal, sacrifice, or a visible mistake.
- Let the scene end after something has changed, not after the narrator explains what it means.
- Preserve quiet scenes by making the pressure intimate or internalized, not by forcing action spectacle.
- Do not force every scene into the same conflict template; choose pressure that belongs to this character, place, and chapter.
- Durable changes in state, knowledge, resources, injuries, locations, or relationships should become memory candidates.
"#;

const WORLDBUILDING_SKILL: &str = r#"---
name: worldbuilding
description: Maintain world rules, institutions, locations, resources, and setting consequences.
---

Use this skill for setting-heavy planning, chapter briefs, continuity audits, and revisions involving world rules.

Rules:
- Treat worldbuilding as constraints that create choices and costs, not encyclopedia exposition.
- Track institutions, geography, technology/magic rules, money/resources, laws/customs, travel time, communication limits, and public knowledge.
- Do not add a new rule, faction, artifact, rank, or technology unless it is compatible with `bible/`, `cards/world/`, `cards/locations/`, memory ledgers, and prior chapters.
- If a rule is bent or broken, state the cost, witness, exception, or future consequence.
- Prefer placing setting information inside conflict, transaction, ritual, obstacle, rumor, or mistake.
- Do not use a fixed encyclopedia template when the scene only needs one precise constraint or consequence.
- Durable world facts should become candidates for `memory/facts.jsonl` or world/location cards with evidence.
"#;

const XIANXIA_CRAFT_SKILL: &str = r#"---
name: xianxia-craft
description: Draft, audit, or revise Chinese xianxia/xuanhuan scenes with grounded cultivation rules, resource economics, combat knowledge loops, and human texture while preserving canon.
---

Use this skill for xianxia, xuanhuan, cultivation, sect, artifact, realm, spell, resource, or immortal-world chapter work.

Rules:
- Treat `book.toml`, `bible/`, `cards/`, chapters, and `memory/` as canon; do not invent realms, sect rules, artifacts, prices, or secret knowledge that canon does not support.
- Convert emotion into body, object, silence, interruption, delayed action, or a costly choice before naming the feeling.
- When a resource matters, anchor it with value, scarcity, ordinary income, debt, faction obligation, risk, or a visible tradeoff.
- In combat, make victory depend on knowledge: early pressure, concrete observation, exploited flaw, then a fast reversal. Do not turn every fight into raw power comparison.
- Let worldbuilding surface through action, bargaining, ritual, travel, injury, payment, rank pressure, taboo, or dialogue under leverage.
- Dialogue should expose class, sect position, age, fear, debt, education, and current leverage; avoid giving every cultivator the same formal voice.
- Use short breath sentences for impact when the scene asks for it, but do not force a fixed rhythm into every paragraph.
- For audits, name only actionable continuity or craft gaps and prefer fixes that preserve chapter shape. Do not force a universal beat sheet, template, or formula.

Original micro-patterns:
- Resource anchor: "One pill solves the scene only if the chapter also names what it costs, who cannot afford it, and what obligation follows."
- Combat knowledge loop: "The first exchange hurts the viewpoint character; the second gives a sensory clue; the final move spends that clue."
- World rule through action: "A junior stops at the gate, pays, bleeds, bows, or lies. The rule is understood through the consequence."
- Character voice: "A sect heir threatens through etiquette; a wandering cultivator counts debt; a child names the wrong detail; a defeated elder protects face."
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn skill_file(tmp: &TempDir) -> std::path::PathBuf {
        tmp.path().join("skill-creator").join("SKILL.md")
    }

    fn named_skill_file(tmp: &TempDir, name: &str) -> std::path::PathBuf {
        tmp.path().join(name).join("SKILL.md")
    }

    fn marker_file(tmp: &TempDir) -> std::path::PathBuf {
        tmp.path().join(".system-installed-version")
    }

    fn bundled_skill_body(name: &str) -> &'static str {
        BUNDLED_SYSTEM_SKILLS
            .iter()
            .find_map(|(skill_name, body)| (*skill_name == name).then_some(*body))
            .unwrap_or_else(|| panic!("missing bundled skill {name}"))
    }

    // ── fresh install ─────────────────────────────────────────────────────────

    #[test]
    fn fresh_install_creates_skill_and_marker() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();

        assert!(skill_file(&tmp).exists(), "SKILL.md should be created");
        for (name, _) in BUNDLED_SYSTEM_SKILLS {
            assert!(
                named_skill_file(&tmp, name).exists(),
                "{name}/SKILL.md should be created"
            );
        }
        assert!(marker_file(&tmp).exists(), "marker should be created");

        let ver = fs::read_to_string(marker_file(&tmp)).unwrap();
        assert_eq!(ver.trim(), BUNDLED_SKILL_VERSION);
    }

    #[test]
    fn bundled_writing_skills_cover_plan_targets() {
        let expected = [
            "skill-creator",
            "human-texture",
            "anti-ai-prose",
            "dialogue",
            "suspense",
            "character-arc",
            "webnovel-pacing",
            "scene-pressure",
            "worldbuilding",
            "xianxia-craft",
        ];
        let actual: Vec<_> = BUNDLED_SYSTEM_SKILLS
            .iter()
            .map(|(name, _)| *name)
            .collect();

        for name in expected {
            assert!(
                actual.contains(&name),
                "bundled system skills should include {name}; actual={actual:?}"
            );
        }
    }

    #[test]
    fn bundled_novel_skills_have_frontmatter_and_preserve_canon() {
        for (name, body) in BUNDLED_SYSTEM_SKILLS {
            if *name == "skill-creator" {
                continue;
            }
            assert!(
                body.contains(&format!("name: {name}")),
                "{name} frontmatter must match directory name"
            );
            assert!(
                body.contains("description:"),
                "{name} should have model-visible description"
            );
            assert!(
                body.contains("canon")
                    || body.contains("memory")
                    || body.contains("bible/")
                    || body.contains("cards/"),
                "{name} should anchor outputs to durable book facts"
            );
            assert!(
                body.contains("template")
                    || body.contains("formula")
                    || body.contains("beat sheet")
                    || body.contains("universal structure"),
                "{name} should include an anti-formula writing constraint"
            );
        }
    }

    #[test]
    fn new_serial_scene_and_world_skills_target_book_artifacts() {
        let pacing = bundled_skill_body("webnovel-pacing");
        assert!(pacing.contains("serial"));
        assert!(pacing.contains("character knowledge boundaries"));

        let pressure = bundled_skill_body("scene-pressure");
        assert!(pressure.contains("scene"));
        assert!(pressure.contains("memory candidates"));

        let worldbuilding = bundled_skill_body("worldbuilding");
        assert!(worldbuilding.contains("bible/"));
        assert!(worldbuilding.contains("memory/facts.jsonl"));
        assert!(worldbuilding.contains("world/location cards"));

        let xianxia = bundled_skill_body("xianxia-craft");
        assert!(xianxia.contains("cultivation"));
        assert!(xianxia.contains("resource economics"));
        assert!(xianxia.contains("combat knowledge loops"));
        assert!(xianxia.contains("Do not force a universal beat sheet"));
    }

    // ── idempotence ───────────────────────────────────────────────────────────

    #[test]
    fn calling_twice_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();

        // Overwrite SKILL.md with sentinel to detect an undesired second write.
        fs::write(skill_file(&tmp), "sentinel").unwrap();

        install_system_skills(tmp.path()).unwrap();

        let contents = fs::read_to_string(skill_file(&tmp)).unwrap();
        assert_eq!(
            contents, "sentinel",
            "second install should not overwrite SKILL.md when version is current"
        );
    }

    // ── user deleted the directory ────────────────────────────────────────────

    #[test]
    fn user_deleted_dir_is_not_recreated() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();

        // Simulate user deliberately removing the skill directory.
        fs::remove_dir_all(tmp.path().join("skill-creator")).unwrap();

        // Re-launch must NOT recreate the directory.
        install_system_skills(tmp.path()).unwrap();

        assert!(
            !skill_file(&tmp).exists(),
            "skill-creator must not be recreated after user deleted it"
        );
    }

    // ── version bump installs missing skills without clobbering edits ─────────

    #[test]
    fn outdated_marker_installs_missing_skills_without_overwriting_existing() {
        let tmp = TempDir::new().unwrap();

        // Simulate a previous install at a lower version.
        let skill_dir = tmp.path().join("skill-creator");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "user edited content").unwrap();
        fs::write(marker_file(&tmp), "0").unwrap(); // older than BUNDLED_SKILL_VERSION

        install_system_skills(tmp.path()).unwrap();

        let contents = fs::read_to_string(skill_file(&tmp)).unwrap();
        assert_eq!(
            contents, "user edited content",
            "version bump must not overwrite a user-edited bundled skill"
        );
        assert!(
            named_skill_file(&tmp, "xianxia-craft").exists(),
            "version bump should install newly bundled missing skills"
        );

        let ver = fs::read_to_string(marker_file(&tmp)).unwrap();
        assert_eq!(
            ver.trim(),
            BUNDLED_SKILL_VERSION,
            "marker should be updated"
        );
    }

    // ── uninstall ─────────────────────────────────────────────────────────────

    #[test]
    fn uninstall_removes_skill_and_marker() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();
        uninstall_system_skills(tmp.path()).unwrap();

        assert!(!skill_file(&tmp).exists(), "SKILL.md should be removed");
        assert!(!marker_file(&tmp).exists(), "marker should be removed");
    }

    #[test]
    fn uninstall_on_clean_dir_is_a_noop() {
        let tmp = TempDir::new().unwrap();
        // Must not panic or error.
        uninstall_system_skills(tmp.path()).unwrap();
    }
}
