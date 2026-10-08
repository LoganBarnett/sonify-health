module MetricHistoryTest exposing (suite)

import Expect
import MetricHistory
import Test exposing (Test, describe, test)


suite : Test
suite =
    describe "MetricHistory.push"
        [ test "appends the newest value last" <|
            \_ ->
                MetricHistory.push 0.5 [ 0.1, 0.2 ]
                    |> Expect.equal [ 0.1, 0.2, 0.5 ]
        , test "keeps the newest points once the window is full" <|
            \_ ->
                let
                    polls =
                        List.range 1 (MetricHistory.capacity + 50)
                            |> List.map toFloat

                    history =
                        List.foldl MetricHistory.push [] polls
                in
                Expect.all
                    [ List.length >> Expect.equal MetricHistory.capacity
                    , List.head >> Expect.equal (Just 51)
                    , List.reverse
                        >> List.head
                        >> Expect.equal (List.head (List.reverse polls))
                    ]
                    history
        ]
