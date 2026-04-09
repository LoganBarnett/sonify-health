module Protocol exposing
    ( BoopSpecInfo
    , BoopSpecRanges
    , CheckInfo
    , CheckLogEntry
    , DroneInfo
    , PatchParam
    , ServerMsg(..)
    , decodeServerMsg
    , encodeClearBoopPin
    , encodeClearDronePin
    , encodeClearOverride
    , encodeExportPatch
    , encodeGetState
    , encodeImportConfig
    , encodeLockDrone
    , encodeLockParam
    , encodeOverrideCheck
    , encodeOverrideDrone
    , encodeRevertAll
    , encodeSetBoopCount
    , encodeSetBoopSpec
    , encodeSetDroneBoops
    , encodeSetDroneFreq
    , encodeSetDroneInterpCurve
    , encodeSetDronePhraseGap
    , encodeSetDroneRepeatCurve
    , encodeSetDroneRepeatRate
    , encodeSetDroneSpec
    , encodeSetDroneVolume
    , encodeSetHeartbeatLoop
    , encodeSetHeartbeatVolume
    , encodeSetMasterVolume
    , encodeSetMuted
    , encodeSetPatchParam
    , encodeTriggerHeartbeat
    , encodeUnlockAll
    , encodeUnlockDrone
    , encodeUnlockParam
    )

import Json.Decode as D
import Json.Encode as E


type alias PatchParam =
    { name : String
    , description : String
    , value : Float
    , min : Float
    , max : Float
    , step : Float
    }


type alias CheckInfo =
    { name : String
    , severity : String
    , overridden : Bool
    }


type alias DroneInfo =
    { name : String
    , value : Float
    , volume : Float
    , repeatRate : Float
    , repeatCurve : Float
    , phraseGap : Float
    , interpCurve : Float
    , boops : Int
    , overridden : Bool
    , patchLo : List PatchParam
    , patchHi : List PatchParam
    , lockedParamsLo : List String
    , lockedParamsHi : List String
    , droneSpecs : List BoopSpecInfo
    , droneSpecRanges : BoopSpecRanges
    }


type alias CheckLogEntry =
    { timestamp : Float
    , layer : String
    , name : String
    , result : String
    , overridden : Bool
    }


type alias BoopSpecInfo =
    { freq : Float
    , duration : Float
    , pinned : Bool
    }


type alias BoopSpecRanges =
    { freqMin : Float
    , freqMax : Float
    , freqStep : Float
    , durationMin : Float
    , durationMax : Float
    , durationStep : Float
    }



-- Server messages (incoming)


type ServerMsg
    = StateMsg
        { patch : List PatchParam
        , muted : Bool
        , masterVolume : Float
        , heartbeatVolume : Float
        , heartbeatLoop : Bool
        , boopCount : Int
        , checks : List CheckInfo
        , drones : List DroneInfo
        , lockedParams : List String
        , lockedDrones : List Int
        , boopSpecs : List BoopSpecInfo
        , boopSpecRanges : BoopSpecRanges
        }
    | ParamChanged String (Maybe Int) String Float
    | MuteChanged Bool
    | VolumeChanged String (Maybe Int) Float
    | OverrideChanged String Int (Maybe String) Bool
    | HeartbeatLoopChanged Bool
    | BoopCountChanged Int
    | DroneConfigChanged Int Int
    | DroneRepeatRateChanged Int Float
    | DroneRepeatCurveChanged Int Float
    | DronePhraseGapChanged Int Float
    | DroneInterpCurveChanged Int Float
    | CheckLog CheckLogEntry
    | PatchExport { toml : String, json : String, nix : String }
    | LockedParamsChanged String (Maybe Int) (List String)
    | LockedDronesChanged (List Int)
    | BoopSpecsChanged (List BoopSpecInfo)
    | DroneSpecsChanged Int (List BoopSpecInfo)
    | ImportError String
    | Connected
    | Disconnected


decodeServerMsg : String -> Maybe ServerMsg
decodeServerMsg raw =
    D.decodeString serverMsgDecoder raw
        |> Result.toMaybe


serverMsgDecoder : D.Decoder ServerMsg
serverMsgDecoder =
    D.field "type" D.string
        |> D.andThen
            (\t ->
                case t of
                    "state" ->
                        stateDecoder

                    "param_changed" ->
                        paramChangedDecoder

                    "mute_changed" ->
                        muteChangedDecoder

                    "volume_changed" ->
                        volumeChangedDecoder

                    "override_changed" ->
                        overrideChangedDecoder

                    "heartbeat_loop_changed" ->
                        heartbeatLoopChangedDecoder

                    "boop_count_changed" ->
                        boopCountChangedDecoder

                    "drone_config_changed" ->
                        droneConfigChangedDecoder

                    "drone_repeat_rate_changed" ->
                        droneRepeatRateChangedDecoder

                    "drone_repeat_curve_changed" ->
                        droneRepeatCurveChangedDecoder

                    "drone_phrase_gap_changed" ->
                        dronePhraseGapChangedDecoder

                    "drone_interp_curve_changed" ->
                        droneInterpCurveChangedDecoder

                    "check_log" ->
                        checkLogDecoder

                    "patch_export" ->
                        patchExportDecoder

                    "locked_params_changed" ->
                        lockedParamsChangedDecoder

                    "locked_drones_changed" ->
                        lockedDronesChangedDecoder

                    "boop_specs_changed" ->
                        boopSpecsChangedDecoder

                    "drone_specs_changed" ->
                        droneSpecsChangedDecoder

                    "import_error" ->
                        importErrorDecoder

                    "connected" ->
                        D.succeed Connected

                    "disconnected" ->
                        D.succeed Disconnected

                    _ ->
                        D.fail ("Unknown server message type: " ++ t)
            )


andMap : D.Decoder a -> D.Decoder (a -> b) -> D.Decoder b
andMap =
    D.map2 (|>)


stateDecoder : D.Decoder ServerMsg
stateDecoder =
    D.field "patch_params" (D.list patchParamMetaDecoder)
        |> D.andThen stateDecoderWithMeta


stateDecoderWithMeta :
    List { name : String, description : String, min : Float, max : Float, step : Float }
    -> D.Decoder ServerMsg
stateDecoderWithMeta metas =
    D.map8
        (\patch muted masterVol hbVol hbLoop boopCount checks drones ->
            \locked lockedDrones boopSpecs ranges ->
                StateMsg
                    { patch = patch
                    , muted = muted
                    , masterVolume = masterVol
                    , heartbeatVolume = hbVol
                    , heartbeatLoop = hbLoop
                    , boopCount = boopCount
                    , checks = checks
                    , drones = drones
                    , lockedParams = locked
                    , lockedDrones = lockedDrones
                    , boopSpecs = boopSpecs
                    , boopSpecRanges = ranges
                    }
        )
        (D.field "patch" patchDecoder
            |> D.map (\vals -> mergePatchParams vals metas)
        )
        (D.field "muted" D.bool)
        (D.field "master_volume" D.float)
        (D.field "heartbeat_volume" D.float)
        (D.field "heartbeat_loop" D.bool)
        (D.field "boop_count" D.int)
        (D.field "checks" (D.list checkInfoDecoder))
        (D.field "drones" (D.list (droneInfoDecoderWithMeta metas)))
        |> andMap (D.field "locked_params" (D.list D.string))
        |> andMap (D.field "locked_drones" (D.list D.int))
        |> andMap (D.field "boop_specs" (D.list boopSpecInfoDecoder))
        |> andMap (D.field "boop_spec_ranges" boopSpecRangesDecoder)


patchDecoder : D.Decoder (List ( String, Float ))
patchDecoder =
    D.keyValuePairs D.float


patchParamMetaDecoder : D.Decoder { name : String, description : String, min : Float, max : Float, step : Float }
patchParamMetaDecoder =
    D.map5 (\n d mn mx s -> { name = n, description = d, min = mn, max = mx, step = s })
        (D.field "name" D.string)
        (D.field "description" D.string)
        (D.field "min" D.float)
        (D.field "max" D.float)
        (D.field "step" D.float)


mergePatchParams :
    List ( String, Float )
    -> List { name : String, description : String, min : Float, max : Float, step : Float }
    -> List PatchParam
mergePatchParams values metas =
    let
        lookup name =
            List.filterMap
                (\( k, v ) ->
                    if k == name then
                        Just v

                    else
                        Nothing
                )
                values
                |> List.head
                |> Maybe.withDefault 0
    in
    List.map
        (\m ->
            { name = m.name
            , description = m.description
            , value = lookup m.name
            , min = m.min
            , max = m.max
            , step = m.step
            }
        )
        metas


checkInfoDecoder : D.Decoder CheckInfo
checkInfoDecoder =
    D.map3 CheckInfo
        (D.field "name" D.string)
        (D.field "severity" D.string)
        (D.field "overridden" D.bool)


droneInfoDecoderWithMeta :
    List { name : String, description : String, min : Float, max : Float, step : Float }
    -> D.Decoder DroneInfo
droneInfoDecoderWithMeta metas =
    D.succeed DroneInfo
        |> andMap (D.field "name" D.string)
        |> andMap (D.field "value" D.float)
        |> andMap (D.field "volume" D.float)
        |> andMap (D.field "repeat_rate" D.float)
        |> andMap (D.field "repeat_curve" D.float)
        |> andMap (D.field "phrase_gap" D.float)
        |> andMap (D.field "interp_curve" D.float)
        |> andMap (D.field "boops" D.int)
        |> andMap (D.field "overridden" D.bool)
        |> andMap
            (D.field "patch_lo" patchDecoder
                |> D.map (\vals -> mergePatchParams vals metas)
            )
        |> andMap
            (D.field "patch_hi" patchDecoder
                |> D.map (\vals -> mergePatchParams vals metas)
            )
        |> andMap (D.field "locked_params_lo" (D.list D.string))
        |> andMap (D.field "locked_params_hi" (D.list D.string))
        |> andMap (D.field "specs" (D.list boopSpecInfoDecoder))
        |> andMap (D.field "spec_ranges" boopSpecRangesDecoder)


paramChangedDecoder : D.Decoder ServerMsg
paramChangedDecoder =
    D.map4 ParamChanged
        (D.field "layer" D.string)
        (D.maybe (D.field "index" D.int))
        (D.field "param" D.string)
        (D.field "value" D.float)


muteChangedDecoder : D.Decoder ServerMsg
muteChangedDecoder =
    D.map MuteChanged (D.field "muted" D.bool)


volumeChangedDecoder : D.Decoder ServerMsg
volumeChangedDecoder =
    D.map3 VolumeChanged
        (D.field "layer" D.string)
        (D.maybe (D.field "index" D.int))
        (D.field "volume" D.float)


overrideChangedDecoder : D.Decoder ServerMsg
overrideChangedDecoder =
    D.map4 OverrideChanged
        (D.field "layer" D.string)
        (D.field "index" D.int)
        (D.maybe
            (D.field "value"
                (D.oneOf
                    [ D.string
                    , D.map String.fromFloat D.float
                    ]
                )
            )
        )
        (D.field "overridden" D.bool)


heartbeatLoopChangedDecoder : D.Decoder ServerMsg
heartbeatLoopChangedDecoder =
    D.map HeartbeatLoopChanged (D.field "enabled" D.bool)


checkLogDecoder : D.Decoder ServerMsg
checkLogDecoder =
    D.map5
        (\ts layer name result overridden ->
            CheckLog
                { timestamp = ts
                , layer = layer
                , name = name
                , result = result
                , overridden = overridden
                }
        )
        (D.field "timestamp" D.float)
        (D.field "layer" D.string)
        (D.field "name" D.string)
        (D.field "result" D.string)
        (D.field "overridden" D.bool)


boopCountChangedDecoder : D.Decoder ServerMsg
boopCountChangedDecoder =
    D.map BoopCountChanged (D.field "count" D.int)


droneConfigChangedDecoder : D.Decoder ServerMsg
droneConfigChangedDecoder =
    D.map2 DroneConfigChanged
        (D.field "index" D.int)
        (D.field "boops" D.int)


droneRepeatRateChangedDecoder : D.Decoder ServerMsg
droneRepeatRateChangedDecoder =
    D.map2 DroneRepeatRateChanged
        (D.field "index" D.int)
        (D.field "rate" D.float)


droneRepeatCurveChangedDecoder : D.Decoder ServerMsg
droneRepeatCurveChangedDecoder =
    D.map2 DroneRepeatCurveChanged
        (D.field "index" D.int)
        (D.field "curve" D.float)


dronePhraseGapChangedDecoder : D.Decoder ServerMsg
dronePhraseGapChangedDecoder =
    D.map2 DronePhraseGapChanged
        (D.field "index" D.int)
        (D.field "gap" D.float)


droneInterpCurveChangedDecoder : D.Decoder ServerMsg
droneInterpCurveChangedDecoder =
    D.map2 DroneInterpCurveChanged
        (D.field "index" D.int)
        (D.field "curve" D.float)


patchExportDecoder : D.Decoder ServerMsg
patchExportDecoder =
    D.map3
        (\t j n -> PatchExport { toml = t, json = j, nix = n })
        (D.field "toml" D.string)
        (D.field "json" D.string)
        (D.field "nix" D.string)


boopSpecInfoDecoder : D.Decoder BoopSpecInfo
boopSpecInfoDecoder =
    D.map3 BoopSpecInfo
        (D.field "freq" D.float)
        (D.field "duration" D.float)
        (D.field "pinned" D.bool)


boopSpecRangesDecoder : D.Decoder BoopSpecRanges
boopSpecRangesDecoder =
    D.map6 BoopSpecRanges
        (D.field "freq_min" D.float)
        (D.field "freq_max" D.float)
        (D.field "freq_step" D.float)
        (D.field "duration_min" D.float)
        (D.field "duration_max" D.float)
        (D.field "duration_step" D.float)


lockedParamsChangedDecoder : D.Decoder ServerMsg
lockedParamsChangedDecoder =
    D.map3 LockedParamsChanged
        (D.field "layer" D.string)
        (D.maybe (D.field "index" D.int))
        (D.field "params" (D.list D.string))


lockedDronesChangedDecoder : D.Decoder ServerMsg
lockedDronesChangedDecoder =
    D.map LockedDronesChanged (D.field "indices" (D.list D.int))


boopSpecsChangedDecoder : D.Decoder ServerMsg
boopSpecsChangedDecoder =
    D.map BoopSpecsChanged (D.field "specs" (D.list boopSpecInfoDecoder))


droneSpecsChangedDecoder : D.Decoder ServerMsg
droneSpecsChangedDecoder =
    D.map2 DroneSpecsChanged
        (D.field "index" D.int)
        (D.field "specs" (D.list boopSpecInfoDecoder))


importErrorDecoder : D.Decoder ServerMsg
importErrorDecoder =
    D.map ImportError (D.field "message" D.string)



-- Client messages (outgoing)


encodeGetState : String
encodeGetState =
    E.object [ ( "type", E.string "get_state" ) ]
        |> E.encode 0


encodeSetPatchParam : String -> Maybe Int -> String -> Float -> String
encodeSetPatchParam layer maybeIndex param value =
    let
        base =
            [ ( "type", E.string "set_patch_param" )
            , ( "layer", E.string layer )
            , ( "param", E.string param )
            , ( "value", E.float value )
            ]

        indexField =
            case maybeIndex of
                Just i ->
                    [ ( "index", E.int i ) ]

                Nothing ->
                    []
    in
    E.object (base ++ indexField)
        |> E.encode 0


encodeSetMuted : Bool -> String
encodeSetMuted muted =
    E.object
        [ ( "type", E.string "set_muted" )
        , ( "muted", E.bool muted )
        ]
        |> E.encode 0


encodeSetMasterVolume : Float -> String
encodeSetMasterVolume vol =
    E.object
        [ ( "type", E.string "set_master_volume" )
        , ( "volume", E.float vol )
        ]
        |> E.encode 0


encodeSetHeartbeatVolume : Float -> String
encodeSetHeartbeatVolume vol =
    E.object
        [ ( "type", E.string "set_heartbeat_volume" )
        , ( "volume", E.float vol )
        ]
        |> E.encode 0


encodeSetDroneVolume : Int -> Float -> String
encodeSetDroneVolume index vol =
    E.object
        [ ( "type", E.string "set_drone_volume" )
        , ( "index", E.int index )
        , ( "volume", E.float vol )
        ]
        |> E.encode 0


encodeSetDroneRepeatRate : Int -> Float -> String
encodeSetDroneRepeatRate index rate =
    E.object
        [ ( "type", E.string "set_drone_repeat_rate" )
        , ( "index", E.int index )
        , ( "rate", E.float rate )
        ]
        |> E.encode 0


encodeSetDroneRepeatCurve : Int -> Float -> String
encodeSetDroneRepeatCurve index curve =
    E.object
        [ ( "type", E.string "set_drone_repeat_curve" )
        , ( "index", E.int index )
        , ( "curve", E.float curve )
        ]
        |> E.encode 0


encodeSetDronePhraseGap : Int -> Float -> String
encodeSetDronePhraseGap index gap =
    E.object
        [ ( "type", E.string "set_drone_phrase_gap" )
        , ( "index", E.int index )
        , ( "gap", E.float gap )
        ]
        |> E.encode 0


encodeSetDroneInterpCurve : Int -> Float -> String
encodeSetDroneInterpCurve index curve =
    E.object
        [ ( "type", E.string "set_drone_interp_curve" )
        , ( "index", E.int index )
        , ( "curve", E.float curve )
        ]
        |> E.encode 0


encodeOverrideCheck : String -> Int -> String -> String
encodeOverrideCheck layer index value =
    E.object
        [ ( "type", E.string "override_check" )
        , ( "layer", E.string layer )
        , ( "index", E.int index )
        , ( "value", E.string value )
        ]
        |> E.encode 0


encodeOverrideDrone : Int -> Float -> String
encodeOverrideDrone index value =
    E.object
        [ ( "type", E.string "override_check" )
        , ( "layer", E.string "drone" )
        , ( "index", E.int index )
        , ( "value", E.float value )
        ]
        |> E.encode 0


encodeClearOverride : String -> Int -> String
encodeClearOverride layer index =
    E.object
        [ ( "type", E.string "clear_override" )
        , ( "layer", E.string layer )
        , ( "index", E.int index )
        ]
        |> E.encode 0


encodeSetBoopCount : Int -> String
encodeSetBoopCount count =
    E.object
        [ ( "type", E.string "set_boop_count" )
        , ( "count", E.int count )
        ]
        |> E.encode 0


encodeSetHeartbeatLoop : Bool -> String
encodeSetHeartbeatLoop enabled =
    E.object
        [ ( "type", E.string "set_heartbeat_loop" )
        , ( "enabled", E.bool enabled )
        ]
        |> E.encode 0


encodeTriggerHeartbeat : String
encodeTriggerHeartbeat =
    E.object [ ( "type", E.string "trigger_heartbeat" ) ]
        |> E.encode 0


encodeRevertAll : String
encodeRevertAll =
    E.object [ ( "type", E.string "revert_all" ) ]
        |> E.encode 0


encodeSetDroneFreq : Int -> Float -> String
encodeSetDroneFreq index freq =
    E.object
        [ ( "type", E.string "set_drone_freq" )
        , ( "index", E.int index )
        , ( "freq", E.float freq )
        ]
        |> E.encode 0


encodeSetDroneBoops : Int -> Int -> String
encodeSetDroneBoops index boops =
    E.object
        [ ( "type", E.string "set_drone_boops" )
        , ( "index", E.int index )
        , ( "boops", E.int boops )
        ]
        |> E.encode 0


encodeExportPatch : String
encodeExportPatch =
    E.object [ ( "type", E.string "export_toml" ) ]
        |> E.encode 0


encodeLockParam : String -> Maybe Int -> String -> String
encodeLockParam layer maybeIndex param =
    let
        base =
            [ ( "type", E.string "lock_param" )
            , ( "layer", E.string layer )
            , ( "param", E.string param )
            ]

        indexField =
            case maybeIndex of
                Just i ->
                    [ ( "index", E.int i ) ]

                Nothing ->
                    []
    in
    E.object (base ++ indexField)
        |> E.encode 0


encodeUnlockParam : String -> Maybe Int -> String -> String
encodeUnlockParam layer maybeIndex param =
    let
        base =
            [ ( "type", E.string "unlock_param" )
            , ( "layer", E.string layer )
            , ( "param", E.string param )
            ]

        indexField =
            case maybeIndex of
                Just i ->
                    [ ( "index", E.int i ) ]

                Nothing ->
                    []
    in
    E.object (base ++ indexField)
        |> E.encode 0


encodeUnlockAll : String
encodeUnlockAll =
    E.object [ ( "type", E.string "unlock_all" ) ]
        |> E.encode 0


encodeLockDrone : Int -> String
encodeLockDrone index =
    E.object
        [ ( "type", E.string "lock_drone" )
        , ( "index", E.int index )
        ]
        |> E.encode 0


encodeUnlockDrone : Int -> String
encodeUnlockDrone index =
    E.object
        [ ( "type", E.string "unlock_drone" )
        , ( "index", E.int index )
        ]
        |> E.encode 0


encodeSetBoopSpec : Int -> Maybe Float -> Maybe Float -> String
encodeSetBoopSpec index maybeFreq maybeDuration =
    let
        base =
            [ ( "type", E.string "set_boop_spec" )
            , ( "index", E.int index )
            ]

        freqField =
            case maybeFreq of
                Just f ->
                    [ ( "freq", E.float f ) ]

                Nothing ->
                    []

        durationField =
            case maybeDuration of
                Just d ->
                    [ ( "duration", E.float d ) ]

                Nothing ->
                    []
    in
    E.object (base ++ freqField ++ durationField)
        |> E.encode 0


encodeClearBoopPin : Int -> String
encodeClearBoopPin index =
    E.object
        [ ( "type", E.string "clear_boop_pin" )
        , ( "index", E.int index )
        ]
        |> E.encode 0


encodeImportConfig : String -> String
encodeImportConfig text =
    E.object
        [ ( "type", E.string "import_config" )
        , ( "text", E.string text )
        ]
        |> E.encode 0


encodeSetDroneSpec : Int -> Int -> Maybe Float -> Maybe Float -> String
encodeSetDroneSpec index noteIndex maybeFreq maybeDuration =
    let
        base =
            [ ( "type", E.string "set_drone_spec" )
            , ( "index", E.int index )
            , ( "note_index", E.int noteIndex )
            ]

        freqField =
            case maybeFreq of
                Just f ->
                    [ ( "freq", E.float f ) ]

                Nothing ->
                    []

        durationField =
            case maybeDuration of
                Just d ->
                    [ ( "duration", E.float d ) ]

                Nothing ->
                    []
    in
    E.object (base ++ freqField ++ durationField)
        |> E.encode 0


encodeClearDronePin : Int -> Int -> String
encodeClearDronePin index noteIndex =
    E.object
        [ ( "type", E.string "clear_drone_pin" )
        , ( "index", E.int index )
        , ( "note_index", E.int noteIndex )
        ]
        |> E.encode 0
