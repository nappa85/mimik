# Fixture rules — tool-loop benchmark unit tests

Original fictional test material for unit tests only.

## FX-1 — Focus

Focus is a whole number from zero through `focus_max`. Spending focus subtracts
the cost from current focus. A cost greater than available focus cannot be paid.

## FX-2 — Outer armor

At most one item whose kind is outer-armor may be equipped at a time. Armor
equals the equipped outer armor's armor value, or zero when none is equipped.
Equipping a new outer armor unequips the previous one.

## FX-3 — Pace

Pace equals six minus armor.

## FX-4 — Riposo breve

Dopo un riposo breve il personaggio recupera la concentrazione; la quantità è
stabilita dal gruppo di gioco.
