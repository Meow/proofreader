--- Cases for the readers that rewrite code into the syntax extensions of LuaJIT 3.0: running
--- this file before and after their fixes must print the same lines.

local lines = {}

local function show(...)
  local parts = {}

  for index = 1, select('#', ...) do
    local value = select(index, ...)
    local kind = type(value)

    if kind == 'table' then
      parts[#parts + 1] = 'table:'..tostring(rawget(value, 'name'))
    elseif kind == 'function' then
      parts[#parts + 1] = 'function'
    else
      parts[#parts + 1] = kind..':'..tostring(value)
    end
  end

  lines[#lines + 1] = table.concat(parts, ' ')
end

local reads = 0
local writes = 0
local tracked = setmetatable({}, {
  __index = function(_, key)
    reads = reads + 1
    return rawget(_G, 'tracked_'..key) or 0
  end,
  __newindex = function(_, key, value)
    writes = writes + 1
    rawset(_G, 'tracked_'..key, value)
  end
})

tracked.count = tracked.count + 1
tracked.count = tracked.count + 2
tracked['count'] = tracked['count'] * 3
show(tracked.count, reads, writes)

local vector = setmetatable({ name = 'vector', x = 1 }, {
  __add = function(a, b)
    return setmetatable({ name = 'sum', x = a.x + (type(b) == 'table' and b.x or b) }, getmetatable(a))
  end,
  __concat = function(a, b)
    return (type(a) == 'table' and a.name or a)..'+'..(type(b) == 'table' and b.name or b)
  end
})
local acc = vector
acc = acc + 2
acc = acc + vector
show(acc.x, acc.name)
local label = vector
label = label..'tail'
show(label)

local counter = { n = 0 }
local function bump()
  counter.n = counter.n + 10
  return 1
end
counter.n = counter.n + bump()
show(counter.n)

local list = { 5, 6 }
local index = 1
list[index] = list[index] - 1
list[1] = list[1] * (2 + 3)
list[2] = list[2] / 4
list[2] = list[2] % 1
show(list[1], list[2])

local function two()
  return 7, 8
end

local total = 1
total = total + (two())
total = total + two()
show(total)

local text = 'a'
text = text..'b'..'c'
text = text..1 ..2
text = text..(total > 0 and 'yes' or 'no')
show(text)

local number = 10
number = number - 2 - 3
number = number - (2 - 3)
number = number ^ 2
number = number + 1 + 2
show(number)

local flags = 5
flags = flags + 1
flags = bit.band(flags, 0xFF)
flags = bit.bor(flags, 8, 16)
flags = bit.bxor(flags, 3)
show(flags, bit.lshift(1, 4), bit.rshift(-1, 28), bit.arshift(-256, 4), bit.bnot(flags))
show(bit.band(2.5, 3), bit.bor(2 ^ 32 + 1, 0), bit.bxor(-1.5, 1), bit.band(2 ^ 53, 1), bit.lshift(1, 33))
show(bit.band(flags + 1, flags - 1), -bit.bnot(1), 'x'..bit.band(3, 1), bit.band(1, 3) == 1, #tostring(bit.bor(1, 2)))
show(bit.band(bit.bor(1, 2), bit.bxor(3, 1)), bit.lshift(bit.band(7, 3), 2) + 1)
show(bit.band(1LL, 3), bit.bor(1ULL, 2))
show(bit.band(flags, two()))
show(bit.band(flags, (two())))
show(math.max(bit.band(15, 6), 1))

local function pick(condition)
  return condition and 'yes' or 'no'
end

show(pick(true), pick(false), pick(nil), pick(0), pick(''))
local calls = 0
local function check(value)
  calls = calls + 1
  return value
end
local chosen = check(false) and 1 or check(2)
show(chosen, calls)
local maybe = check(true) and 1 or nil
local negative = check(false) and -1 or 1
local shaped = check(true) and { name = 'built' } or { name = 'other' }
show(maybe, negative, shaped.name, calls)
local unchanged = check(true) and false or 'fallback'
local kept = check(true) and nil or 'fallback'
show(unchanged, kept)
show((check(1) and 'a' or 'b')..'!', ({ check(nil) and 'x' or 'y' })[1], check(true) and true or false)
local nested = check(true) and 'outer' or check(false) and 'inner' or 'last'
show(nested)

local values = { 3, 1, 2 }
table.sort(values, function(a, b) return a > b end)
show(values[1], values[2], values[3])
local doubled = {}
for position, value in ipairs(values) do
  doubled[position] = (function(x) return x * 2 end)(value)
end
show(doubled[1], doubled[2], doubled[3])
local both = function() return two() end
local first = function() return (two()) end
show(both())
show(first())
local adder = function(a, b) return a + b end
local constant = function() return 42 end
local method = function(self) return self.name end
show(adder(1, 2), constant(), method({ name = 'self' }))
local captured = 5
local closure = function(x) return x + captured end
captured = 6
show(closure(1))
local choose = function(condition) return condition and 'on' or 'off' end
show(choose(true), choose(false))
local apply = function(callback) return callback(3) end
show(apply(function(n) return n * n end))
local ternary_function = check(true) and function() return 'called' end or nil
show(ternary_function())

local big = 1000000
local bigger = 4294967295
local odd = 12345678
local small = 1000
show(big, bigger, odd, small, 100000 + 1, 0x10000)

G_COUNTER = 1
G_COUNTER = G_COUNTER + 1
local coerced = '10'
coerced = coerced + 1
local joined = 1
joined = joined..2
show(G_COUNTER, coerced, joined)

local sum = 0
for step = 1, 5 do
  sum = sum + step
  sum = sum - (step % 2 == 0 and 1 or 0)
end
show(sum)

local handlers = check(true) and { describe = function(o) return o:name() end } or nil
show(handlers.describe({ name = function() return 'named' end }))
local guard = function(value) return value == nil and 'missing' or 'present' end
show(guard(nil), guard(false))
local pair = function(a) return a, a end
show(pair(4))
local varargs = function(...) return select('#', ...) end
show(varargs(1, nil, 3))
local masks = {}
for shift = 0, 3 do
  masks[#masks + 1] = bit.lshift(1, shift) + bit.band(shift, 1)
end
show(masks[1], masks[2], masks[3], masks[4])

for _, line in ipairs(lines) do
  print(line)
end
