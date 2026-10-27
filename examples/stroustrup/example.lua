--- An example plugin: greets players and keeps their score.
-- Every folder of examples/ holds this plugin written in its style;
-- `proofreader examples/<style>` checks it against the .proofreader.yml there.

local PLUGIN = {}
local scores = {}
local ranks = {"Newcomer", "Regular", "Veteran"}
local levels = {[0] = 1, [10] = 2, [100] = 3}

--- Greets a player who joined and adds them to the scoreboard.
-- @param actor [Player the player who joined]
function PLUGIN:PlayerInitialSpawn(actor)
    if (!IsValid(actor)) then return end

    local name = actor:Name()
    local message = "Welcome, " .. name .. "!"

    if (actor:IsAdmin() && name != "") then
        message = message .. " (admin)"
    end

    // Remember the player for the scoreboard.
    scores[actor:SteamID()] = {name = name, points = 0}
    actor:ChatPrint(message)
end

--- Adds points to the score of a player.
-- @param actor [Player the player to reward]
-- @param amount [Number points to add]
-- @return [Number the new score]
function PLUGIN:AddPoints(actor, amount)
    local entry = scores[actor:SteamID()]

    if (!entry) then
        return 0
    end

    entry.points = entry.points + amount

    return entry.points
end

--- Returns the title of the rank a player has reached.
-- @param actor [Player the player]
-- @return [String the rank title]
function PLUGIN:GetRank(actor)
    local entry = scores[actor:SteamID()]

    return ranks[levels[entry.points]] || ranks[1]
end

--- Returns the scoreboard entries, highest score first.
-- @return [Table the sorted entries]
function PLUGIN:GetLeaders()
    local leaders = table.ClearKeys(scores)

    table.sort(leaders, function(a, b)
        return a.points > b.points
    end)

    return leaders
end

return PLUGIN
