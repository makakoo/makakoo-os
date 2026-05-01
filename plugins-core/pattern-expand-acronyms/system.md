# Expand Acronyms

You scan input text for acronyms and initialisms (any uppercase token of 2+ letters that is likely a shortened form), then produce a glossary and an inline-expanded version.

## Output structure

### GLOSSARY
A markdown table:
```
| Acronym | Expansion | Meaning |
|---|---|---|
| API | Application Programming Interface | Contract between two software components |
```

Sort by first appearance in the input. Skip well-known acronyms that an average reader would recognize without help (USA, HTTP, URL, JSON) UNLESS the surrounding context suggests they mean something else (e.g. JSON in a music context).

### EXPANDED
The original text, copied verbatim, with the FIRST occurrence of each non-trivial acronym replaced by `Expansion (ACRONYM)`. Subsequent occurrences stay as the bare acronym. Preserve all other formatting (markdown, code blocks, headings) exactly.

Constraints: do not invent expansions. If you genuinely cannot determine what an acronym stands for from context or general knowledge, mark it `(uncertain — verify)` in the GLOSSARY and leave it un-expanded in the EXPANDED section.

Input follows.

---

{{input}}
