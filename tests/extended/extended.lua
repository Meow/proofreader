--- Cases for the fixes of Lint/ExtendedSyntax: running this file before and after they turn
--- the syntax extensions of LuaJIT 3.0 into plain Lua must print the same lines.

const lines = {}

const function show(...)
  local parts = {}

  for index = 1, select('#', ...) do
    const value = select(index, ...)
    parts[#parts + 1] = type(value)..':'..(type(value) == 'function' and 'function' or tostring(value))
  end

  lines[#lines + 1] = table.concat(parts, ' ')
end

const LIMIT = 1_000_000
local flags = 0x_0F
flags |= 1 << 4
flags &= ~1
flags ~= 0xFF
flags <<= 2
flags >>= 1
flags ~>>= 1
show(LIMIT, flags, 5 & 3 | 8, 1 + 2 & 3, (6 ~ 3) << 1, -16 ~>> 2, ~0)

local counter = { n = 1, list = { 3 } }
counter.n += 2
counter.n *= 3
counter.n -= 1 - 2
counter.n /= 2
counter.n %= 4
counter['n'] ..= '!'
counter.list[1] += 1
local key = 'n'
counter[key] ..= '?'
show(counter.n, counter.list[1])

local function two()
  return 1, 2
end

local value = 10
value += two()
value -= (two())
show(value, two() & 3, 1 | two())

const condition = true
show(condition ? 'yes' : 'no', nil ? 1 : 2, false ? 'a' : nil, condition ? false : true)
show(condition ? 1 : condition ? 2 : 3, not condition ? 1 : 2, condition and false ? 1 : 2)
show(1 + 1 == 2 ? 'math' : 'broken', condition ? -1 : 1, (condition ? 'a' : 'b')..'!')

const object = { name = 'object', get = function(self) return self.name end }
show(object?.name, object?.missing?.deeper, object?.:get(), object:get?.(), nil ?? 'default', false ?? 'kept')

const double = x -> x * 2
const add = |a, b| -> a + b
const count = |...| -> select('#', ...)
const both = || -> two()
const block = |a| -> do
  local doubled = a * 2

  return doubled + 1
end
show(double(4), add(1, 2), count(nil, nil), both())
show(block(3), condition ? |x| -> x + 1 : nil, (|x| -> x)(7))

;; local spaced = 1;;
show(spaced)

for _, line in ipairs(lines) do
  print(line)
end
