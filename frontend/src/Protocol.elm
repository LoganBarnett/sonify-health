module Protocol exposing
    ( BoopSpecInfo
    , BoopSpecRanges
    , CheckInfo
    , CheckLogEntry
    , DroneInfo
    , ServerMsg(..)
    , VoiceParam
    , decodeServerMsg
    , encodeClearBoopPin
    , encodeClearOverride
    , encodeExportToml
    , encodeGetState
    , encodeLockDrone
    , encodeLockParam
    , encodeOverrideCheck
    , encodeOverrideDrone
    , encodeRevertAll
    , encodeSetBoopCount
    , encodeSetBoopSpec
    , encodeSetDroneRegister
    , encodeSetDroneTexture
    , encodeSetDroneVolume
    , encodeSetHeartbeatLoop
    , encodeSetHeartbeatVolume
    , encodeSetMuted
    , encodeSetVoiceParam
    , encodeTriggerHeartbeat
    , encodeUnlockAll
    , encodeUnlockDrone
    , encodeUnlockParam
    )

import Json.Decode as D
import Json.Encode as E


type alias VoiceParam =
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
    , texture : String
    , register : String
    , overridden : Bool
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
        { voice : List VoiceParam
        , muted : Bool
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
    | ParamChanged String Float
    | MuteChanged Bool
    | VolumeChanged String (Maybe Int) Float
    | OverrideChanged String Int (Maybe String) Bool
    | HeartbeatLoopChanged Bool
    | BoopCountChanged Int
    | DroneConfigChanged Int String String
    | CheckLog CheckLogEntry
    | TomlExport String
    | LockedParamsChanged (List String)
    | LockedDronesChanged (List Int)
    | BoopSpecsChanged (List BoopSpecInfo)
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

                    "check_log" ->
                        checkLogDecoder

                    "toml_export" ->
                        tomlExportDecoder

                    "locked_params_changed" ->
                        lockedParamsChangedDecoder

                    "locked_drones_changed" ->
                        lockedDronesChangedDecoder

                    "boop_specs_changed" ->
                        boopSpecsChangedDecoder

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
    D.map7
        (\voice muted hbVol hbLoop boopCount checks drones ->
            \locked lockedDrones boopSpecs ranges ->
                StateMsg
                    { voice = voice
                    , muted = muted
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
        (D.field "voice" voiceDecoder
            |> D.andThen
                (\voiceValues ->
                    D.field "voice_params" (D.list voiceParamMetaDecoder)
                        |> D.map (mergeVoiceParams voiceValues)
                )
        )
        (D.field "muted" D.bool)
        (D.field "heartbeat_volume" D.float)
        (D.field "heartbeat_loop" D.bool)
        (D.field "boop_count" D.int)
        (D.field "checks" (D.list checkInfoDecoder))
        (D.field "drones" (D.list droneInfoDecoder))
        |> andMap (D.field "locked_params" (D.list D.string))
        |> andMap (D.field "locked_drones" (D.list D.int))
        |> andMap (D.field "boop_specs" (D.list boopSpecInfoDecoder))
        |> andMap (D.field "boop_spec_ranges" boopSpecRangesDecoder)


voiceDecoder : D.Decoder (List ( String, Float ))
voiceDecoder =
    D.keyValuePairs D.float


voiceParamMetaDecoder : D.Decoder { name : String, description : String, min : Float, max : Float, step : Float }
voiceParamMetaDecoder =
    D.map5 (\n d mn mx s -> { name = n, description = d, min = mn, max = mx, step = s })
        (D.field "name" D.string)
        (D.field "description" D.string)
        (D.field "min" D.float)
        (D.field "max" D.float)
        (D.field "step" D.float)


mergeVoiceParams :
    List ( String, Float )
    -> List { name : String, description : String, min : Float, max : Float, step : Float }
    -> List VoiceParam
mergeVoiceParams values metas =
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


droneInfoDecoder : D.Decoder DroneInfo
droneInfoDecoder =
    D.map6 DroneInfo
        (D.field "name" D.string)
        (D.field "value" D.float)
        (D.field "volume" D.float)
        (D.field "texture" D.string)
        (D.field "register" D.string)
        (D.field "overridden" D.bool)


paramChangedDecoder : D.Decoder ServerMsg
paramChangedDecoder =
    D.map2 ParamChanged
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
    D.map3 DroneConfigChanged
        (D.field "index" D.int)
        (D.field "texture" D.string)
        (D.field "register" D.string)


tomlExportDecoder : D.Decoder ServerMsg
tomlExportDecoder =
    D.map TomlExport (D.field "content" D.string)


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
    D.map LockedParamsChanged (D.field "params" (D.list D.string))


lockedDronesChangedDecoder : D.Decoder ServerMsg
lockedDronesChangedDecoder =
    D.map LockedDronesChanged (D.field "indices" (D.list D.int))


boopSpecsChangedDecoder : D.Decoder ServerMsg
boopSpecsChangedDecoder =
    D.map BoopSpecsChanged (D.field "specs" (D.list boopSpecInfoDecoder))



-- Client messages (outgoing)


encodeGetState : String
encodeGetState =
    E.object [ ( "type", E.string "get_state" ) ]
        |> E.encode 0


encodeSetVoiceParam : String -> Float -> String
encodeSetVoiceParam param value =
    E.object
        [ ( "type", E.string "set_voice_param" )
        , ( "param", E.string param )
        , ( "value", E.float value )
        ]
        |> E.encode 0


encodeSetMuted : Bool -> String
encodeSetMuted muted =
    E.object
        [ ( "type", E.string "set_muted" )
        , ( "muted", E.bool muted )
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


encodeSetDroneTexture : Int -> String -> String
encodeSetDroneTexture index texture =
    E.object
        [ ( "type", E.string "set_drone_texture" )
        , ( "index", E.int index )
        , ( "texture", E.string texture )
        ]
        |> E.encode 0


encodeSetDroneRegister : Int -> String -> String
encodeSetDroneRegister index register =
    E.object
        [ ( "type", E.string "set_drone_register" )
        , ( "index", E.int index )
        , ( "register", E.string register )
        ]
        |> E.encode 0


encodeExportToml : String
encodeExportToml =
    E.object [ ( "type", E.string "export_toml" ) ]
        |> E.encode 0


encodeLockParam : String -> String
encodeLockParam param =
    E.object
        [ ( "type", E.string "lock_param" )
        , ( "param", E.string param )
        ]
        |> E.encode 0


encodeUnlockParam : String -> String
encodeUnlockParam param =
    E.object
        [ ( "type", E.string "unlock_param" )
        , ( "param", E.string param )
        ]
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
