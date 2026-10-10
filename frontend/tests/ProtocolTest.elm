module ProtocolTest exposing (suite)

import Expect
import Json.Decode as D
import Protocol
import Test exposing (Test, describe, test)


meta : String -> Result D.Error Protocol.PatchParamMeta
meta limits =
    D.decodeString Protocol.patchParamMetaDecoder
        ("""{"name": "attack_ms", "description": "", "min": 0, "max": 500,
            "step": 1, "logarithmic": false"""
            ++ limits
            ++ "}"
        )


suite : Test
suite =
    describe "Protocol.patchParamMetaDecoder"
        [ test "decodes present hard limits" <|
            \_ ->
                meta """, "limit_min": 0, "limit_max": 1"""
                    |> Result.map (\m -> ( m.limitMin, m.limitMax ))
                    |> Expect.equal (Ok ( Just 0, Just 1 ))
        , test "reads a null limit as unbounded" <|
            \_ ->
                meta """, "limit_min": 0, "limit_max": null"""
                    |> Result.map (\m -> ( m.limitMin, m.limitMax ))
                    |> Expect.equal (Ok ( Just 0, Nothing ))
        , test "reads absent limits as unbounded" <|
            \_ ->
                meta ""
                    |> Result.map (\m -> ( m.limitMin, m.limitMax ))
                    |> Expect.equal (Ok ( Nothing, Nothing ))
        ]
