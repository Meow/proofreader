--- An example plugin: greets players and keeps their score.
-- Every folder of examples/ holds this plugin written in its style;
-- `proofreader examples/<style>` checks it against the .proofreader.yml there.

local PLUGIN = {}
local scores = {}

--- Greets a player who joined and adds them to the scoreboard.
-- @param actor [Player the player who joined]
function PLUGIN:PlayerInitialSpawn(actor)
  if !IsValid(actor) then return end

  local name = actor:Name()
  local message = 'Welcome, '..name..'!'

  if actor:IsAdmin() and name != '' then
    message = message..' (admin)'
  end

  -- Remember the player for the scoreboard.
  scores[actor:SteamID()] = { name = name, points = 0 }
  actor:ChatPrint(message)
end

--- Adds points to the score of a player.
-- @param actor [Player the player to reward]
-- @param amount [Number points to add]
-- @return [Number the new score]
function PLUGIN:AddPoints(actor, amount)
  local entry = scores[actor:SteamID()]

  if !entry then
    return 0
  end

  entry.points = entry.points + amount

  return entry.points
end

return PLUGIN
