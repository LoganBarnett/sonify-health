module Help exposing (Key, Source(..), lookup)

{-| Short explanations for labels and controls in the UI.

Every clickable help label in the UI passes a `Key` (a stable
string) into `HelpClicked`; the main view then calls `lookup` to
render the corresponding text in the bottom help panel.

Adding a new labeled control? Add a help entry here. See
`CONTRIBUTING.org` for the Definition of Done.

-}


{-| A stable, short string identifying a help entry. By convention
we use kebab-case (`"metric-tiers"`, `"patch-detune"`) so keys are
easy to grep for in both Elm and CSS.
-}
type alias Key =
    String


{-| Where help text comes from.

`Registered` points at an entry in this module's `lookup` table —
use this for labels whose explanation is authored here in the
frontend (section headers, static controls).

`Literal` carries the text verbatim — use this for labels whose
explanation is dynamic and arrives from the backend (for example
per-patch-parameter `description` fields coming over the
WebSocket, which we don't want to duplicate in `lookup`).

Unifying both under one type lets `activeHelp` and `HelpClicked`
stay a single field / message regardless of source.

-}
type Source
    = Registered Key
    | Literal String


{-| Look up the help text for a key. Returns `Nothing` if the key
isn't registered — in which case the UI falls back to a "no help
available" placeholder so missing entries don't break the panel.
Keep entries short: one or two sentences. The panel is small by
design; long prose belongs in `docs/`.
-}
lookup : Key -> Maybe String
lookup key =
    case key of
        "metric-tiers" ->
            Just "Cosmetic thresholds for the metric badge — labels and colors only. They don't affect audio; the metric value itself drives synthesis through each note's transition."

        _ ->
            Nothing
