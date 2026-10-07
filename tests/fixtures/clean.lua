--- Greets the given player.
-- @param actor [Player]
function greet(actor)
  if !IsValid(actor) then return end

  actor:notify('Hello, '..actor:name()..'!')
end
