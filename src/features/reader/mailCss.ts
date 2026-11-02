import {
  resolveMailResourceUrl,
  type ResourceContext,
} from "./mailResourceUrls.ts";

function decodeEscape(
  value: string,
  index: number,
): { value: string; end: number } {
  const match = value.slice(index + 1).match(/^[\da-f]{1,6}/i);
  if (match) {
    const code = Number.parseInt(match[0], 16);
    let end = index + 1 + match[0].length;
    if (/\s/.test(value[end] ?? "")) end++;
    return {
      value:
        code > 0 && code <= 0x10ffff ? String.fromCodePoint(code) : "\ufffd",
      end,
    };
  }
  const next = value[index + 1] ?? "";
  if (next === "\r" || next === "\n" || next === "\f")
    return {
      value: "",
      end: index + (next === "\r" && value[index + 2] === "\n" ? 3 : 2),
    };
  return { value: next, end: index + 2 };
}

function decodeCss(value: string): string {
  let result = "";
  for (let index = 0; index < value.length;) {
    if (value[index] === "\\") {
      const escaped = decodeEscape(value, index);
      result += escaped.value;
      index = escaped.end;
    } else result += value[index++];
  }
  return result;
}

function quotedEnd(value: string, start: number): number {
  const quote = value[start];
  let index = start + 1;
  while (index < value.length) {
    if (value[index] === "\\") index = decodeEscape(value, index).end;
    else if (value[index++] === quote) return index;
  }
  return value.length;
}

function functionEnd(value: string, start: number): number {
  let depth = 1;
  for (let index = start + 1; index < value.length; index++) {
    if (value[index] === '"' || value[index] === "'")
      index = quotedEnd(value, index) - 1;
    else if (value[index] === "\\") index = decodeEscape(value, index).end - 1;
    else if (value[index] === "(") depth++;
    else if (value[index] === ")" && --depth === 0) return index;
  }
  return value.length;
}

function cssString(value: string): string {
  return `"${value
    .replace(/\\/g, "\\\\")
    .replace(/"/g, '\\"')
    .replace(/[\r\n\f]/g, "")}"`;
}

/** 解析 CSS 的函数/字符串边界，支持转义的 url()、image-set() 与 MIME CID。 */
export function rewriteCssUrls(
  value: string,
  context: ResourceContext,
  used: Set<number> = new Set(),
  imageStrings = false,
  depth = 0,
): string {
  if (depth > 40) return "none";
  const image = (raw: string) => {
    const resource = resolveMailResourceUrl(raw, context);
    if (!resource) return "none";
    if (resource.attachmentIndex !== null) used.add(resource.attachmentIndex);
    return `url(${cssString(resource.url)})`;
  };
  let result = "";
  for (let index = 0; index < value.length;) {
    if (value.startsWith("/*", index)) {
      const end = value.indexOf("*/", index + 2);
      index = end < 0 ? value.length : end + 2;
      result += " ";
      continue;
    }
    if (value[index] === '"' || value[index] === "'") {
      const end = quotedEnd(value, index);
      result += imageStrings
        ? image(decodeCss(value.slice(index + 1, end - 1)))
        : value.slice(index, end);
      index = end;
      continue;
    }
    if (/[a-z_-]/i.test(value[index]) || value[index] === "\\") {
      const start = index;
      let name = "";
      while (
        index < value.length &&
        (/[\w-]/.test(value[index]) || value[index] === "\\")
      ) {
        if (value[index] === "\\") {
          const escaped = decodeEscape(value, index);
          name += escaped.value;
          index = escaped.end;
        } else name += value[index++];
      }
      let opening = index;
      while (/\s/.test(value[opening] ?? "")) opening++;
      if (value[opening] !== "(") {
        result += value.slice(start, index);
        continue;
      }
      const end = functionEnd(value, opening);
      let inner = value.slice(opening + 1, end).trim();
      const lower = name.toLowerCase();
      if (lower === "url") {
        if (
          (inner.startsWith('"') && inner.endsWith('"')) ||
          (inner.startsWith("'") && inner.endsWith("'"))
        )
          inner = inner.slice(1, -1);
        result += image(decodeCss(inner));
      } else if (lower === "expression" || lower === "attr") result += "none";
      else
        result += `${name}(${rewriteCssUrls(inner, context, used, /^(?:-webkit-)?image-set$|^image$/.test(lower), depth + 1)})`;
      index = end < value.length ? end + 1 : end;
      continue;
    }
    result += value[index++];
  }
  return result;
}

function rewriteDeclarations(
  style: CSSStyleDeclaration,
  context: ResourceContext,
  used: Set<number>,
): string {
  const declarations: string[] = [];
  for (let index = 0; index < style.length; index++) {
    const property = style.item(index);
    if (/^(?:behavior|-moz-binding)$/i.test(property)) continue;
    const value = rewriteCssUrls(
      style.getPropertyValue(property),
      context,
      used,
    );
    declarations.push(
      `${property}:${value}${style.getPropertyPriority(property) ? " !important" : ""};`,
    );
  }
  return declarations.join("");
}

export function sanitizeInlineStyle(
  value: string,
  context: ResourceContext,
  used: Set<number>,
): string {
  const style = document.createElement("span").style;
  style.cssText = value;
  return rewriteDeclarations(style, context, used);
}

/** CSSOM 保留合法布局规则与媒体查询，但不挂载 stylesheet，避免解析时发起请求。 */
export function sanitizeStyleSheet(
  value: string,
  context: ResourceContext,
  used: Set<number>,
): string {
  const sheet = new CSSStyleSheet();
  try {
    sheet.replaceSync(value);
  } catch {
    return "";
  }
  const rules = (entries: CSSRuleList, depth = 0): string => {
    if (depth > 30) return "";
    return Array.from(entries)
      .map((rule) => {
        if (
          rule.type === CSSRule.IMPORT_RULE ||
          rule.type === CSSRule.NAMESPACE_RULE
        )
          return "";
        if (rule instanceof CSSStyleRule)
          return `${rule.selectorText}{${rewriteDeclarations(rule.style, context, used)}}`;
        if (rule instanceof CSSFontFaceRule) {
          // 邮件字体族/字号照常保留；外部字体不作为图片下载，local() 字体仍可使用。
          const original = rule.style.getPropertyValue("src");
          if (/url\s*\(/i.test(original) || /\\/.test(original))
            rule.style.removeProperty("src");
          return `@font-face{${rewriteDeclarations(rule.style, context, used)}}`;
        }
        if ("cssRules" in rule) {
          const group = rule as CSSRule & { cssRules: CSSRuleList };
          return `${rule.cssText.slice(0, rule.cssText.indexOf("{"))}{${rules(group.cssRules, depth + 1)}}`;
        }
        if ("style" in rule) {
          const styled = rule as CSSRule & { style: CSSStyleDeclaration };
          return `${rule.cssText.slice(0, rule.cssText.indexOf("{"))}{${rewriteDeclarations(styled.style, context, used)}}`;
        }
        return /^@(?:layer|charset)\b/i.test(rule.cssText) ? rule.cssText : "";
      })
      .join("\n");
  };
  return rules(sheet.cssRules).replace(/<\/style/gi, "<\\/style");
}
