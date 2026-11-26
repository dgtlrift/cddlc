//! The runtime emitted into every generated Lua module: the item model, type
//! checks, and a self-contained CBOR or JSON codec. Lua 5.3+ (integer
//! subtype, bitwise operators, `string.pack`, `utf8`, `math.ult`).

/// The item model and checks, shared by both formats.
pub const COMMON: &str = r#"-- ── Runtime ───────────────────────────────────────────────────────────────────
--
-- Every value converts to an item { k = kind, v = value }, so text and bytes
-- (both Lua strings) and integers and floats stay apart: kinds are uint, nint,
-- float, bool, null, text, bytes, array, map and tag. A uint above 2^63 is held
-- as its 64-bit pattern (a negative Lua integer) and compared with math.ult.

local R = {}

--- A CDDL nil: CBOR or JSON null. Lua's nil can't be stored in a table.
local NULL = setmetatable({}, { __name = "cddl.null", __tostring = function() return "null" end })
M.null = NULL

local function fail(at, msg) error(at .. ": " .. msg, 0) end
R.fail = fail

function R.uint(n) return { k = "uint", v = n } end
function R.int(n) if n < 0 then return { k = "nint", v = n } end return { k = "uint", v = n } end
function R.f32(x) return { k = "float", v = x + 0.0, w = 32 } end
function R.f64(x) return { k = "float", v = x + 0.0, w = 64 } end
function R.bool(b) return { k = "bool", v = b } end
function R.null() return { k = "null" } end
function R.text(s) return { k = "text", v = s } end
function R.bytes(s) return { k = "bytes", v = s } end
function R.array(items) return { k = "array", v = items } end
--- A map: a list of { key, item } pairs, in the order they are written.
function R.map(pairs) return { k = "map", v = pairs } end

local KINDS = {
  uint = "an unsigned integer", nint = "a negative integer", float = "a float", bool = "a bool",
  null = "null", text = "text", bytes = "a byte string", array = "an array", map = "a map", tag = "a tag",
}

function R.kind_name(item) return KINDS[item.k] or "an unknown value" end

local function mismatch(at, want, item) fail(at, "expected " .. want .. ", got " .. R.kind_name(item)) end
R.mismatch = mismatch

-- ── Items to Lua values, each kind checked ─────────────────────────────────────

function R.as_uint(item, at)
  if item.k ~= "uint" then mismatch(at, "an unsigned integer", item) end
  return item.v
end

function R.as_int(item, at)
  if item.k == "nint" or (item.k == "uint" and item.v >= 0) then return item.v end
  if item.k == "uint" then fail(at, "integer out of range") end
  mismatch(at, "an integer", item)
end

function R.as_float(item, at)
  if item.k == "float" then return item.v end
  if item.k == "nint" then return item.v + 0.0 end
  if item.k == "uint" then
    if item.v < 0 then return (item.v >> 1) * 2.0 + (item.v & 1) end -- above 2^63
    return item.v + 0.0
  end
  mismatch(at, "a number", item)
end

function R.as_bool(item, at)
  if item.k ~= "bool" then mismatch(at, "a bool", item) end
  return item.v
end

function R.as_text(item, at)
  if item.k ~= "text" then mismatch(at, "text", item) end
  return item.v
end

function R.as_null(item, at)
  if item.k ~= "null" then mismatch(at, "null", item) end
  return NULL
end

function R.as_array(item, at)
  if item.k ~= "array" then mismatch(at, "an array", item) end
  return item.v
end

--- A map item as a table from text key to item.
function R.as_map(item, at)
  if item.k ~= "map" then mismatch(at, "a map", item) end
  local m = {}
  for _, p in ipairs(item.v) do
    local key = p[1]
    -- Keys are read as text: an integer key as its decimal text (dgtlrift/cddlc#176).
    local kv
    if key.k == "text" then kv = key.v
    elseif key.k == "uint" or key.k == "nint" then kv = string.format("%d", key.v)
    else fail(at, "map keys must be text or integers") end
    if m[kv] ~= nil then fail(at, "duplicate key \"" .. kv .. "\"") end
    m[kv] = p[2]
  end
  return m
end

function R.required(m, key, at)
  local item = m[key]
  if item == nil then fail(at, "missing member \"" .. key .. "\"") end
  return item
end

--- Whether an optional member is present (null counts as absent).
function R.present(m, key)
  local item = m[key]
  return item ~= nil and item.k ~= "null"
end

-- ── Checks on Lua values before encoding ───────────────────────────────────────

local function lua_kind(x) return math.type(x) or type(x) end

function R.want_integer(x, at)
  if math.type(x) ~= "integer" then fail(at, "expected an integer, got " .. lua_kind(x)) end
end

function R.want_number(x, at)
  if type(x) ~= "number" then fail(at, "expected a number, got " .. lua_kind(x)) end
end

function R.want_boolean(x, at)
  if type(x) ~= "boolean" then fail(at, "expected a boolean, got " .. lua_kind(x)) end
end

function R.want_string(x, at)
  if type(x) ~= "string" then fail(at, "expected a string, got " .. lua_kind(x)) end
end

function R.want_text(x, at)
  R.want_string(x, at)
  if not utf8.len(x) then fail(at, "text is not valid UTF-8") end
end

function R.want_null(x, at)
  if x ~= NULL then fail(at, "expected null (M.null), got " .. lua_kind(x)) end
end

function R.want_table(x, at)
  if type(x) ~= "table" then fail(at, "expected a table, got " .. lua_kind(x)) end
end

--- a <= b, unsigned.
function R.ule(a, b) return a == b or math.ult(a, b) end

-- ── Any value (an inline type choice) ──────────────────────────────────────────

function R.from_lua(x)
  local t = math.type(x)
  if t == "integer" then return R.int(x) end
  if t == "float" then return R.f64(x) end
  if type(x) == "string" then return R.text(x) end
  if type(x) == "boolean" then return R.bool(x) end
  if x == NULL then return R.null() end
  fail("value", "cannot encode " .. lua_kind(x))
end

function R.to_lua(item)
  local k = item.k
  if k == "uint" or k == "nint" or k == "float" or k == "bool" or k == "text" or k == "bytes" then return item.v end
  if k == "null" then return NULL end
  return item
end
"#;

/// CBOR (RFC 8949): definite lengths, maps in the order given, float32 and
/// float64 at their own widths, tags kept.
pub const CBOR: &str = r#"
-- ── CBOR codec ────────────────────────────────────────────────────────────────

local spack, sunpack, schar = string.pack, string.unpack, string.char

function R.tag(n, item) return { k = "tag", n = n, v = item } end

function R.untag(item, n, at)
  if item.k ~= "tag" or item.n ~= n then fail(at, "expected CBOR tag " .. n) end
  return item.v
end

function R.as_bytes(item, at)
  if item.k ~= "bytes" then mismatch(at, "a byte string", item) end
  return item.v
end

-- An item head: major type and its (unsigned) argument.
local function head(major, n)
  local m = major << 5
  if math.ult(n, 24) then return schar(m | n) end
  if math.ult(n, 0x100) then return schar(m | 24, n) end
  if math.ult(n, 0x10000) then return schar(m | 25) .. spack(">I2", n) end
  if math.ult(n, 0x100000000) then return schar(m | 26) .. spack(">I4", n) end
  return schar(m | 27) .. spack(">i8", n) -- the 64-bit pattern
end

local function enc(item, out)
  local k = item.k
  if k == "uint" then
    out[#out + 1] = head(0, item.v)
  elseif k == "nint" then
    out[#out + 1] = head(1, -1 - item.v)
  elseif k == "float" then
    if item.w == 32 then out[#out + 1] = "\xfa" .. spack(">f", item.v)
    else out[#out + 1] = "\xfb" .. spack(">d", item.v) end
  elseif k == "bool" then
    out[#out + 1] = item.v and "\xf5" or "\xf4"
  elseif k == "null" then
    out[#out + 1] = "\xf6"
  elseif k == "text" then
    out[#out + 1] = head(3, #item.v)
    out[#out + 1] = item.v
  elseif k == "bytes" then
    out[#out + 1] = head(2, #item.v)
    out[#out + 1] = item.v
  elseif k == "array" then
    out[#out + 1] = head(4, #item.v)
    for _, x in ipairs(item.v) do enc(x, out) end
  elseif k == "map" then
    out[#out + 1] = head(5, #item.v)
    for _, p in ipairs(item.v) do
      -- A key is text, or an integer key as an item (dgtlrift/cddlc#176).
      if type(p[1]) == "table" then
        enc(p[1], out)
      else
        out[#out + 1] = head(3, #p[1])
        out[#out + 1] = p[1]
      end
      enc(p[2], out)
    end
  elseif k == "tag" then
    out[#out + 1] = head(6, item.n)
    enc(item.v, out)
  else
    error("cbor: cannot encode a " .. tostring(k), 0)
  end
end

--- An item as CBOR bytes.
function R.encode(item)
  local out = {}
  enc(item, out)
  return table.concat(out)
end

local function need(s, pos, n)
  if pos + n - 1 > #s then error("cbor: unexpected end of input", 0) end
end

local function read_arg(s, pos, ai)
  if ai < 24 then return ai, pos end
  if ai == 24 then need(s, pos, 1) return s:byte(pos), pos + 1 end
  if ai == 25 then need(s, pos, 2) return sunpack(">I2", s, pos) end
  if ai == 26 then need(s, pos, 4) return sunpack(">I4", s, pos) end
  if ai == 27 then need(s, pos, 8) return sunpack(">i8", s, pos) end
  error("cbor: invalid additional information " .. ai, 0)
end

local function half(h)
  local e, m = (h >> 10) & 0x1f, h & 0x3ff
  local v
  if e == 0 then v = m * 2.0 ^ -24
  elseif e == 31 then v = (m == 0) and math.huge or (0.0 / 0.0)
  else v = (m + 1024) * 2.0 ^ (e - 25) end
  if h & 0x8000 ~= 0 then v = -v end
  return v
end

local dec

-- A byte or text string, definite or in chunks.
local function read_string(s, pos, major, ai)
  if ai == 31 then
    local parts = {}
    while true do
      need(s, pos, 1)
      local ib = s:byte(pos)
      if ib == 0xff then return table.concat(parts), pos + 1 end
      if ib >> 5 ~= major then error("cbor: invalid chunk in an indefinite-length string", 0) end
      local part
      part, pos = read_string(s, pos + 1, major, ib & 31)
      parts[#parts + 1] = part
    end
  end
  local n
  n, pos = read_arg(s, pos, ai)
  if n < 0 then error("cbor: string too long", 0) end
  need(s, pos, n)
  return s:sub(pos, pos + n - 1), pos + n
end

dec = function(s, pos, depth)
  if depth > 64 then error("cbor: nesting too deep", 0) end
  need(s, pos, 1)
  local ib = s:byte(pos)
  pos = pos + 1
  local major, ai = ib >> 5, ib & 31
  if major == 0 then
    local n
    n, pos = read_arg(s, pos, ai)
    return { k = "uint", v = n }, pos
  elseif major == 1 then
    local n
    n, pos = read_arg(s, pos, ai)
    if n < 0 then error("cbor: negative integer out of range", 0) end
    return { k = "nint", v = -1 - n }, pos
  elseif major == 2 or major == 3 then
    local str
    str, pos = read_string(s, pos, major, ai)
    if major == 3 then
      if not utf8.len(str) then error("cbor: text is not valid UTF-8", 0) end
      return { k = "text", v = str }, pos
    end
    return { k = "bytes", v = str }, pos
  elseif major == 4 or major == 5 then
    local items = {}
    local n
    if ai == 31 then n = -1 else n, pos = read_arg(s, pos, ai) end
    local i = 0
    while true do
      if n < 0 then
        need(s, pos, 1)
        if s:byte(pos) == 0xff then pos = pos + 1 break end
      elseif i == n then
        break
      end
      i = i + 1
      if major == 4 then
        items[i], pos = dec(s, pos, depth + 1)
      else
        local key, val
        key, pos = dec(s, pos, depth + 1)
        val, pos = dec(s, pos, depth + 1)
        items[i] = { key, val }
      end
    end
    return { k = major == 4 and "array" or "map", v = items }, pos
  elseif major == 6 then
    local n, inner
    n, pos = read_arg(s, pos, ai)
    inner, pos = dec(s, pos, depth + 1)
    return { k = "tag", n = n, v = inner }, pos
  end
  if ai == 20 then return { k = "bool", v = false }, pos end
  if ai == 21 then return { k = "bool", v = true }, pos end
  if ai == 22 or ai == 23 then return { k = "null" }, pos end
  if ai == 25 then need(s, pos, 2) local h = sunpack(">I2", s, pos) return { k = "float", v = half(h), w = 16 }, pos + 2 end
  if ai == 26 then need(s, pos, 4) local f = sunpack(">f", s, pos) return { k = "float", v = f, w = 32 }, pos + 4 end
  if ai == 27 then need(s, pos, 8) local d = sunpack(">d", s, pos) return { k = "float", v = d, w = 64 }, pos + 8 end
  error("cbor: unsupported simple value " .. ai, 0)
end

--- CBOR bytes as an item.
function R.decode(s)
  if type(s) ~= "string" then error("cbor: expected a string of bytes", 0) end
  local item, pos = dec(s, 1, 0)
  if pos <= #s then error("cbor: trailing data after the value", 0) end
  return item
end
"#;

/// JSON (RFC 8259): byte strings are base64url text without padding, and JSON
/// has no tags.
pub const JSON: &str = r#"
-- ── JSON codec ────────────────────────────────────────────────────────────────

local spack, sunpack = string.pack, string.unpack

--- JSON has no tags: the item is written as is.
function R.tag(_, item) return item end

--- JSON has no tags: the item is read as is.
function R.untag(item) return item end

local B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
local B64INV = {}
for i = 1, 64 do B64INV[B64:byte(i)] = i - 1 end

local function b64url(s)
  local out = {}
  for i = 1, #s, 3 do
    local a, b, c = s:byte(i, i + 2)
    local n = (a << 16) | ((b or 0) << 8) | (c or 0)
    local chars = 2 + (b and 1 or 0) + (c and 1 or 0)
    for j = 1, chars do
      local x = (n >> (18 - 6 * (j - 1))) & 63
      out[#out + 1] = B64:sub(x + 1, x + 1)
    end
  end
  return table.concat(out)
end

local function unb64url(s, at)
  s = s:gsub("=+$", "")
  local out, bits, nbits = {}, 0, 0
  for i = 1, #s do
    local x = B64INV[s:byte(i)]
    if x == nil then fail(at, "invalid base64url") end
    bits, nbits = ((bits << 6) | x) & 0xffffff, nbits + 6
    if nbits >= 8 then
      nbits = nbits - 8
      out[#out + 1] = string.char((bits >> nbits) & 0xff)
    end
  end
  if nbits >= 6 then fail(at, "invalid base64url length") end
  return table.concat(out)
end

--- Bytes are base64url text in JSON.
function R.bytes(s) return { k = "text", v = b64url(s) } end

function R.as_bytes(item, at)
  if item.k ~= "text" then mismatch(at, "base64url text", item) end
  return unb64url(item.v, at)
end

local ESC = { ['"'] = '\\"', ["\\"] = "\\\\", ["\b"] = "\\b", ["\f"] = "\\f", ["\n"] = "\\n", ["\r"] = "\\r", ["\t"] = "\\t" }

local function json_str(s)
  return '"' .. s:gsub('[%c"\\]', function(c) return ESC[c] or string.format("\\u%04x", c:byte()) end) .. '"'
end

local function fmt_uint(n)
  if n >= 0 then return string.format("%d", n) end
  local q = (n >> 1) // 5 -- n / 10, unsigned
  return string.format("%d", q) .. string.format("%d", n - q * 10)
end

-- The shortest decimal that reads back as the same float (float32 or float64).
local function fmt_float(x, w)
  if x ~= x or x == math.huge or x == -math.huge then error("json: cannot encode " .. tostring(x), 0) end
  local s
  if w == 32 then
    x = sunpack(">f", spack(">f", x))
    for p = 6, 9 do
      s = string.format("%." .. p .. "g", x)
      if sunpack(">f", spack(">f", tonumber(s))) == x then break end
    end
  else
    for p = 15, 17 do
      s = string.format("%." .. p .. "g", x)
      if tonumber(s) == x then break end
    end
  end
  if not s:find("[.eE]") then s = s .. ".0" end
  return s
end

local function enc(item, out)
  local k = item.k
  if k == "uint" then
    out[#out + 1] = fmt_uint(item.v)
  elseif k == "nint" then
    out[#out + 1] = string.format("%d", item.v)
  elseif k == "float" then
    out[#out + 1] = fmt_float(item.v, item.w)
  elseif k == "bool" then
    out[#out + 1] = item.v and "true" or "false"
  elseif k == "null" then
    out[#out + 1] = "null"
  elseif k == "text" then
    out[#out + 1] = json_str(item.v)
  elseif k == "array" then
    out[#out + 1] = "["
    for i, x in ipairs(item.v) do
      if i > 1 then out[#out + 1] = "," end
      enc(x, out)
    end
    out[#out + 1] = "]"
  elseif k == "map" then
    out[#out + 1] = "{"
    for i, p in ipairs(item.v) do
      if i > 1 then out[#out + 1] = "," end
      out[#out + 1] = json_str(p[1])
      out[#out + 1] = ":"
      enc(p[2], out)
    end
    out[#out + 1] = "}"
  else
    error("json: cannot encode a " .. tostring(k), 0)
  end
end

--- An item as JSON text.
function R.encode(item)
  local out = {}
  enc(item, out)
  return table.concat(out)
end

local function ws(s, pos) return s:find("[^ \t\r\n]", pos) or #s + 1 end

local function jerr(msg, pos) error("json: " .. msg .. " at byte " .. pos, 0) end

local function read_string(s, pos)
  local out = {}
  pos = pos + 1
  while true do
    local c = s:sub(pos, pos)
    if c == "" then jerr("unterminated string", pos) end
    if c == '"' then return table.concat(out), pos + 1 end
    if c:byte() < 0x20 then jerr("control character in string", pos) end
    if c == "\\" then
      local e = s:sub(pos + 1, pos + 1)
      local simple = ({ ['"'] = '"', ["\\"] = "\\", ["/"] = "/", b = "\b", f = "\f", n = "\n", r = "\r", t = "\t" })[e]
      if simple then
        out[#out + 1] = simple
        pos = pos + 2
      elseif e == "u" then
        local cp = tonumber(s:sub(pos + 2, pos + 5), 16)
        if not cp then jerr("invalid \\u escape", pos) end
        pos = pos + 6
        if cp >= 0xd800 and cp <= 0xdbff and s:sub(pos, pos + 1) == "\\u" then
          local lo = tonumber(s:sub(pos + 2, pos + 5), 16)
          if lo and lo >= 0xdc00 and lo <= 0xdfff then
            cp = 0x10000 + ((cp - 0xd800) << 10) + (lo - 0xdc00)
            pos = pos + 6
          end
        end
        out[#out + 1] = utf8.char(cp)
      else
        jerr("invalid escape", pos)
      end
    else
      out[#out + 1] = c
      pos = pos + 1
    end
  end
end

local U64MAX = "18446744073709551615"

local function read_number(s, pos)
  local int = s:match("^-?%d+", pos)
  if not int then jerr("invalid value", pos) end
  local p = pos + #int
  local frac = s:match("^%.%d+", p) or ""
  p = p + #frac
  local exp = s:match("^[eE][-+]?%d+", p) or ""
  p = p + #exp
  if frac ~= "" or exp ~= "" then
    return { k = "float", v = tonumber(int .. frac .. exp), w = 64 }, p
  end
  local n = math.tointeger(tonumber(int))
  if n and string.format("%d", n) == int then return R.int(n), p end
  if int:sub(1, 1) == "-" or #int > #U64MAX or (#int == #U64MAX and int > U64MAX) then
    jerr("integer out of range", pos)
  end
  n = 0
  for i = 1, #int do n = n * 10 + (int:byte(i) - 48) end -- wraps to the 64-bit pattern
  return { k = "uint", v = n }, p
end

local function dec(s, pos, depth)
  if depth > 64 then jerr("nesting too deep", pos) end
  pos = ws(s, pos)
  local c = s:sub(pos, pos)
  if c == "{" then
    local pairs_ = {}
    pos = ws(s, pos + 1)
    if s:sub(pos, pos) == "}" then return { k = "map", v = pairs_ }, pos + 1 end
    while true do
      pos = ws(s, pos)
      if s:sub(pos, pos) ~= '"' then jerr("expected a member name", pos) end
      local key, val
      key, pos = read_string(s, pos)
      pos = ws(s, pos)
      if s:sub(pos, pos) ~= ":" then jerr("expected ':'", pos) end
      val, pos = dec(s, pos + 1, depth + 1)
      pairs_[#pairs_ + 1] = { { k = "text", v = key }, val }
      pos = ws(s, pos)
      local d = s:sub(pos, pos)
      if d == "}" then return { k = "map", v = pairs_ }, pos + 1 end
      if d ~= "," then jerr("expected ',' or '}'", pos) end
      pos = pos + 1
    end
  elseif c == "[" then
    local items = {}
    pos = ws(s, pos + 1)
    if s:sub(pos, pos) == "]" then return { k = "array", v = items }, pos + 1 end
    while true do
      items[#items + 1], pos = dec(s, pos, depth + 1)
      pos = ws(s, pos)
      local d = s:sub(pos, pos)
      if d == "]" then return { k = "array", v = items }, pos + 1 end
      if d ~= "," then jerr("expected ',' or ']'", pos) end
      pos = pos + 1
    end
  elseif c == '"' then
    local str
    str, pos = read_string(s, pos)
    return { k = "text", v = str }, pos
  elseif s:sub(pos, pos + 3) == "true" then
    return { k = "bool", v = true }, pos + 4
  elseif s:sub(pos, pos + 4) == "false" then
    return { k = "bool", v = false }, pos + 5
  elseif s:sub(pos, pos + 3) == "null" then
    return { k = "null" }, pos + 4
  end
  return read_number(s, pos)
end

--- JSON text as an item.
function R.decode(s)
  if type(s) ~= "string" then error("json: expected a string", 0) end
  local item, pos = dec(s, 1, 0)
  if ws(s, pos) <= #s then jerr("trailing data after the value", pos) end
  return item
end
"#;
