module MetricHistory exposing (capacity, push)

{-| The rolling window of metric values a heartbeat's sparkline plots.
-}


{-| Points the window holds: the most recent polls.
-}
capacity : Int
capacity =
    99


{-| `history` with `value` as its newest point, the oldest points dropped to
stay within `capacity`.
-}
push : Float -> List Float -> List Float
push value history =
    let
        combined =
            history ++ [ value ]
    in
    List.drop (List.length combined - capacity) combined
