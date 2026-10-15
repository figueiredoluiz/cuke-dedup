/** Mask Rust comments and literals while preserving offsets and newlines for function ranges. */
export function rustCode(source) {
  const chars = source.split("");
  /** Blank a lexical construct without shifting subsequent source locations. */
  function blank(start, end) {
    for (let j = start; j < end; j++) if (chars[j] !== "\n") chars[j] = " ";
    return end;
  }
  for (let i = 0; i < source.length;) {
    if (source.startsWith("//", i)) {
      const end = source.indexOf("\n", i);
      i = blank(i, end < 0 ? source.length : end);
    } else if (source.startsWith("/*", i)) {
      let end = i + 2, depth = 1;
      while (end < source.length && depth) {
        if (source.startsWith("/*", end)) { depth++; end += 2; }
        else if (source.startsWith("*/", end)) { depth--; end += 2; }
        else end++;
      }
      if (depth) throw new Error("Unterminated Rust block comment");
      i = blank(i, end);
    } else {
      const raw = /^(?:br|cr|r)(#{0,255})"/.exec(source.slice(i));
      const char = /^'(?:\\(?:u\{[\da-fA-F_]+\}|x[\da-fA-F]{2}|.)|[^'\\\n])'/u.exec(source.slice(i));
      if (raw) {
        const close = '"' + raw[1];
        const end = source.indexOf(close, i + raw[0].length);
        if (end < 0) throw new Error("Unterminated Rust raw string");
        i = blank(i, end + close.length);
      } else if (source[i] === '"') {
        let end = i + 1;
        while (end < source.length && source[end] !== '"') end += source[end] === "\\" ? 2 : 1;
        if (end >= source.length) throw new Error("Unterminated Rust string");
        i = blank(i, end + 1);
      } else if (char) i = blank(i, i + char[0].length);
      else i++;
    }
  }
  return chars.join("");
}
