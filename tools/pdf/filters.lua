-- Pandoc Lua filter: the book/ Markdown -> the Typst body that tools/pdf/doc.typ lays out.
-- Copied unchanged (apart from this header) from the reference course's PDF pipeline.
--
--  * "# Chapter 2 — Title"    -> a level-1 heading numbered "2" with the supplement
--                               [Chapter]; the template sets "CHAPTER 2" above the title
--  * "## 2.1 Title"           -> a heading numbered "2.1" ("## 1. Title" -> "1",
--    "### 2.1.3 Title"           "## A. Title" -> "A"): the number is drawn in the accent
--                               colour and listed as the number in the contents and the
--                               PDF outline
--  * metadata `idprefix`      -> (the course book, one chapter at a time) every heading id
--    and `chapterid`             gets the prefix, the chapter title the id `chapterid`, so
--                               ids stay unique across the book; "#frag" links follow
--  * metadata `chapters`      -> (the book) a link to another chapter's file becomes a
--                               link to that chapter's heading in the book
--  * a "#frag" link to a heading that does not exist -> its text, and a warning: Typst
--    refuses to compile a link to a missing label
--  * fence info strings: "rust,ignore" -> rust, jsonc -> json, console/shell/zsh -> bash
--  * table columns size to their content instead of the widths pandoc derives from the
--    dashes in the separator row (which are all `---` here, i.e. equal widths)

local idprefix = ""
local chapterid = nil
local chapters = {} -- "02-ownership.md" -> "02-ownership"
local ids = {}

local function typst_inlines(inlines)
  local s = pandoc.write(pandoc.Pandoc({ pandoc.Plain(inlines) }), "typst")
  return (s:gsub("%s+$", ""))
end

local function typst_string(s)
  return '"' .. s:gsub("\\", "\\\\"):gsub('"', '\\"') .. '"'
end

function Meta(meta)
  if meta.idprefix then
    idprefix = pandoc.utils.stringify(meta.idprefix)
  end
  if meta.chapterid then
    chapterid = pandoc.utils.stringify(meta.chapterid)
  end
  if meta.chapters then
    for stem in pandoc.utils.stringify(meta.chapters):gmatch("[^,]+") do
      chapters[stem .. ".md"] = stem
    end
  end
end

-- Pass 1: final heading ids, collected so that pass 2 can check links against them.
local function assign_id(h)
  if h.level == 1 and chapterid then
    h.identifier = chapterid
  elseif h.identifier ~= "" then
    h.identifier = idprefix .. h.identifier
  end
  ids[h.identifier] = true
  return h
end

-- Pass 2: links.
local function link(l)
  local path, frag = l.target:match("^([^#]*)#?(.*)$")
  if path == "" and frag ~= "" then
    local id = idprefix .. frag
    if ids[id] then
      l.target = "#" .. id
      return l
    end
    io.stderr:write(
      ("warning: link to #%s (%q): no heading has that id\n"):format(frag, pandoc.utils.stringify(l.content))
    )
    return l.content
  end
  local stem = chapters[path]
  if stem then
    local id = frag ~= "" and (stem .. "-" .. frag) or stem
    return pandoc.RawInline("typst", "#xref(" .. typst_string(id) .. ")[" .. typst_inlines(l.content) .. "]")
  end
  return nil
end

local LANG = { jsonc = "json", console = "bash", shell = "bash", zsh = "bash", sh = "bash" }

local function code_block(cb)
  local info = cb.classes[1]
  if not info then
    return nil
  end
  local lang = info:match("^[^,%s]+")
  lang = LANG[lang] or lang
  if lang == info then
    return nil
  end
  cb.classes[1] = lang
  return cb
end

local function table_auto_widths(t)
  for i, cs in ipairs(t.colspecs) do
    t.colspecs[i] = { cs[1], nil }
  end
  return t
end

-- Pass 3: a heading whose text starts with its number becomes a Typst heading that
-- carries the number as its numbering, so the template can style the two apart.
local function strip(inlines, n)
  local rest = pandoc.List()
  for i = n + 1, #inlines do
    rest:insert(inlines[i])
  end
  return rest
end

local function heading(h)
  local c = h.content
  local number, supplement, body
  if
    h.level == 1
    and #c > 6
    and c[1].t == "Str" and c[1].text == "Chapter"
    and c[3].t == "Str" and c[3].text:match("^%d+$")
    and c[5].t == "Str" and c[5].text == "—"
  then
    number, supplement, body = c[3].text, "Chapter", strip(c, 6)
  elseif h.level > 1 and #c > 2 and c[1].t == "Str" and c[2].t == "Space" then
    local s = c[1].text
    if s:match("^%d+%.[%.%d]*$") then
      number = s:gsub("%.$", "")
    elseif s:match("^[A-Z]%.$") then
      number = s:sub(1, 1)
    end
    body = strip(c, 2)
  end
  if not number then
    return nil
  end
  local args = ("level: %d, numbering: (..n) => %s"):format(h.level, typst_string(number))
  if supplement then
    args = args .. ", supplement: [" .. supplement .. "]"
  end
  local label = h.identifier ~= "" and (" #label(" .. typst_string(h.identifier) .. ")") or ""
  return pandoc.RawBlock("typst", "#heading(" .. args .. ")[" .. typst_inlines(body) .. "]" .. label)
end

return {
  { Meta = Meta },
  { Header = assign_id },
  { Link = link, CodeBlock = code_block, Table = table_auto_widths },
  { Header = heading },
}
