/**
 * Minimal SKILL.md frontmatter parser. Expects:
 *
 *   ---
 *   name: my-skill
 *   description: "..."
 *   ---
 *   # body
 *
 * Only `name` and `description` are required. Other YAML keys are ignored.
 * No external YAML dependency — keeps the skill loader self-contained.
 */

export interface ParsedSkillMd {
  name: string;
  description: string;
  body: string;
}

const MAX_BODY_CHARS = 100_000;

/** Parses a SKILL.md file. Returns null when required fields are missing/invalid. */
export function parseSkillMd(raw: string): ParsedSkillMd | null {
  if (typeof raw !== "string" || !raw.trim()) return null;

  const normalized = raw.replace(/^\uFEFF/, "");
  if (!normalized.startsWith("---")) return null;

  const end = normalized.indexOf("\n---", 3);
  if (end === -1) return null;

  const frontmatter = normalized.slice(3, end).replace(/^\r?\n/, "");
  let body = normalized.slice(end + 4).replace(/^\r?\n/, "");
  if (body.length > MAX_BODY_CHARS) {
    body = body.slice(0, MAX_BODY_CHARS) + "\n\n[skill body truncated]";
  }

  const fields = parseFrontmatterFields(frontmatter);
  const name = normalizeName(fields.name);
  const description = normalizeDescription(fields.description);
  if (!name || !description) return null;

  return { name, description, body };
}

function parseFrontmatterFields(frontmatter: string): Record<string, string> {
  const fields: Record<string, string> = {};
  const lines = frontmatter.split(/\r?\n/);
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const match = line.match(/^([A-Za-z][A-Za-z0-9_-]*)\s*:\s*(.*)$/);
    if (!match) {
      i++;
      continue;
    }
    const key = match[1];
    let value = match[2] ?? "";
    // Folded/literal block scalars are uncommon in skill frontmatter; support a
    // simple multi-line quoted or plain continuation only for "description".
    if ((value.startsWith('"') && !value.endsWith('"')) || (value.startsWith("'") && !value.endsWith("'"))) {
      const quote = value[0];
      const parts = [value.slice(1)];
      i++;
      while (i < lines.length) {
        const cont = lines[i];
        if (cont.endsWith(quote) && !cont.endsWith("\\" + quote)) {
          parts.push(cont.slice(0, -1));
          break;
        }
        parts.push(cont);
        i++;
      }
      value = parts.join("\n");
    } else if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
      value = value.slice(1, -1);
    }
    fields[key] = value.replace(/\\n/g, "\n").trim();
    i++;
  }
  return fields;
}

/** Skill names are lowercase slugs: letters, digits, hyphen, underscore, slash for nested. */
function normalizeName(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const name = value.trim().toLowerCase();
  if (!name || name.length > 64) return null;
  if (!/^[a-z0-9][a-z0-9/_-]*$/.test(name)) return null;
  if (name.includes("..") || name.startsWith("/") || name.endsWith("/")) return null;
  return name;
}

function normalizeDescription(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const description = value.trim().replace(/\s+/g, " ");
  if (!description || description.length > 1024) return null;
  return description;
}

export { MAX_BODY_CHARS };
