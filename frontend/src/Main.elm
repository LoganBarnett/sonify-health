module Main exposing (main)

import Browser
import Browser.Navigation as Nav
import Dict exposing (Dict)
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (onClick, onInput)
import Http
import Json.Decode as Decode
import Ports
import Process
import Protocol exposing (..)
import Set exposing (Set)
import Task
import Url exposing (Url)
import Url.Parser exposing (Parser)


type Route
    = Home
    | Me
    | NotFound


routeParser : Parser (Route -> a) a
routeParser =
    Url.Parser.oneOf
        [ Url.Parser.map Home Url.Parser.top
        , Url.Parser.map Me (Url.Parser.s "me")
        ]


routeFromUrl : Url -> Route
routeFromUrl url =
    Url.Parser.parse routeParser url
        |> Maybe.withDefault NotFound


type alias MeInfo =
    { name : String
    , authEnabled : Bool
    }


type MeStatus
    = MeLoading
    | MeLoaded MeInfo
    | MeFailed


type alias Model =
    { key : Nav.Key
    , url : Url
    , route : Route
    , me : MeStatus
    , connected : Bool
    , patch : List PatchParam
    , muted : Bool
    , masterVolume : Float
    , heartbeatVolume : Float
    , heartbeatLoop : Bool
    , boopCount : Int
    , checks : List CheckInfo
    , checkLog : List CheckLogEntry
    , exportData : Maybe { toml : String, json : String, nix : String }
    , exportTab : String
    , debounces : Dict String Int
    , nextDebounce : Int
    , lockedParams : Set String
    , lockedDrones : Set Int
    , boopSpecs : List BoopSpecInfo
    , boopSpecRanges : BoopSpecRanges
    , importText : String
    , importError : Maybe String
    }


type Msg
    = UrlRequested Browser.UrlRequest
    | UrlChanged Url
    | GotMe (Result Http.Error MeInfo)
    | WebSocketReceived String
    | SetPatchParam String (Maybe Int) String String
    | PatchDebounce String (Maybe Int) String Int Float
    | ToggleMute
    | SetMasterVolume String
    | MasterVolDebounce Int Float
    | SetHeartbeatVolume String
    | HeartbeatVolDebounce Int Float
    | SetBoopCount String
    | BoopCountDebounce Int Int
    | SetDroneInterpCurve Int String
    | DroneInterpCurveDebounce Int Int Float
    | OverrideCheck Int String
    | ClearCheckOverride Int
    | SetDroneBoops Int String
    | OverrideDroneValue Int String
    | ClearDroneOverride Int
    | ToggleHeartbeatLoop
    | TriggerHeartbeat
    | RevertAll
    | Export
    | DismissExport
    | SetExportTab String
    | ToggleLockParam String (Maybe Int) String
    | ToggleLockDrone Int
    | UnlockAll
    | SetBoopFreq Int String
    | BoopFreqDebounce Int Int Float
    | SetBoopDuration Int String
    | BoopDurationDebounce Int Int Float
    | ClearBoopPin Int
    | SetDroneNoteFreq Int Int String
    | DroneNoteFreqDebounce Int Int Int Float
    | SetDroneNoteDuration Int Int String
    | DroneNoteDurationDebounce Int Int Int Float
    | ClearDronePin Int Int
    | SetImportText String
    | SubmitImport
    | NoOp


main : Program () Model Msg
main =
    Browser.application
        { init = init
        , view = view
        , update = update
        , subscriptions = subscriptions
        , onUrlRequest = UrlRequested
        , onUrlChange = UrlChanged
        }


init : () -> Url -> Nav.Key -> ( Model, Cmd Msg )
init _ url key =
    let
        route =
            routeFromUrl url
    in
    ( { key = key
      , url = url
      , route = route
      , me = MeLoading
      , connected = False
      , patch = []
      , muted = False
      , masterVolume = 1.0
      , heartbeatVolume = 1.0
      , heartbeatLoop = False
      , boopCount = 1
      , checks = []
      , checkLog = []
      , exportData = Nothing
      , exportTab = "toml"
      , debounces = Dict.empty
      , nextDebounce = 0
      , lockedParams = Set.empty
      , lockedDrones = Set.empty
      , boopSpecs = []
      , boopSpecRanges =
            { freqMin = 50.0
            , freqMax = 12000.0
            , freqStep = 1.0
            , durationMin = 0.05
            , durationMax = 1.2
            , durationStep = 0.01
            }
      , importText = ""
      , importError = Nothing
      }
    , cmdForRoute route
    )


cmdForRoute : Route -> Cmd Msg
cmdForRoute route =
    case route of
        Me ->
            fetchMe

        _ ->
            Cmd.none


fetchMe : Cmd Msg
fetchMe =
    Http.get
        { url = "/me"
        , expect = Http.expectJson GotMe meDecoder
        }


meDecoder : Decode.Decoder MeInfo
meDecoder =
    Decode.map2 MeInfo
        (Decode.field "name" Decode.string)
        (Decode.field "auth_enabled" Decode.bool)


subscriptions : Model -> Sub Msg
subscriptions _ =
    Ports.websocketReceive WebSocketReceived



-- UPDATE


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        UrlRequested (Browser.Internal url) ->
            ( model, Nav.pushUrl model.key (Url.toString url) )

        UrlRequested (Browser.External url) ->
            ( model, Nav.load url )

        UrlChanged url ->
            let
                route =
                    routeFromUrl url
            in
            ( { model | url = url, route = route, me = MeLoading }
            , cmdForRoute route
            )

        GotMe result ->
            case result of
                Ok info ->
                    ( { model | me = MeLoaded info }, Cmd.none )

                Err _ ->
                    ( { model | me = MeFailed }, Cmd.none )

        WebSocketReceived raw ->
            handleServerMsg raw model

        SetPatchParam layer maybeIndex name valStr ->
            case String.toFloat valStr of
                Just value ->
                    let
                        key =
                            patchDebounceKey layer maybeIndex name

                        id =
                            model.nextDebounce

                        updateParam p =
                            if p.name == name then
                                { p | value = value }

                            else
                                p
                    in
                    case ( layer, maybeIndex ) of
                        ( "drone_lo", Just i ) ->
                            ( { model
                                | checks =
                                    updateCheckByKindIndex "drone"
                                        i
                                        (\c -> { c | patchLo = List.map updateParam c.patchLo })
                                        model.checks
                                , debounces = Dict.insert key id model.debounces
                                , nextDebounce = id + 1
                              }
                            , Process.sleep 50
                                |> Task.perform
                                    (\_ -> PatchDebounce layer maybeIndex name id value)
                            )

                        ( "drone_hi", Just i ) ->
                            ( { model
                                | checks =
                                    updateCheckByKindIndex "drone"
                                        i
                                        (\c -> { c | patchHi = List.map updateParam c.patchHi })
                                        model.checks
                                , debounces = Dict.insert key id model.debounces
                                , nextDebounce = id + 1
                              }
                            , Process.sleep 50
                                |> Task.perform
                                    (\_ -> PatchDebounce layer maybeIndex name id value)
                            )

                        _ ->
                            ( { model
                                | patch = List.map updateParam model.patch
                                , debounces = Dict.insert key id model.debounces
                                , nextDebounce = id + 1
                              }
                            , Process.sleep 50
                                |> Task.perform
                                    (\_ -> PatchDebounce layer maybeIndex name id value)
                            )

                Nothing ->
                    ( model, Cmd.none )

        PatchDebounce layer maybeIndex name id value ->
            let
                key =
                    patchDebounceKey layer maybeIndex name
            in
            if Dict.get key model.debounces == Just id then
                ( model
                , Ports.websocketSend
                    (encodeSetPatchParam layer maybeIndex name value)
                )

            else
                ( model, Cmd.none )

        ToggleMute ->
            let
                newMuted =
                    not model.muted
            in
            ( { model | muted = newMuted }
            , Ports.websocketSend (encodeSetMuted newMuted)
            )

        SetMasterVolume valStr ->
            case String.toFloat valStr of
                Just vol ->
                    let
                        id =
                            model.nextDebounce
                    in
                    ( { model
                        | masterVolume = vol
                        , nextDebounce = id + 1
                      }
                    , Process.sleep 50
                        |> Task.perform (\_ -> MasterVolDebounce id vol)
                    )

                Nothing ->
                    ( model, Cmd.none )

        MasterVolDebounce id vol ->
            if id == model.nextDebounce - 1 || True then
                ( model
                , Ports.websocketSend (encodeSetMasterVolume vol)
                )

            else
                ( model, Cmd.none )

        SetHeartbeatVolume valStr ->
            case String.toFloat valStr of
                Just vol ->
                    let
                        id =
                            model.nextDebounce
                    in
                    ( { model
                        | heartbeatVolume = vol
                        , nextDebounce = id + 1
                      }
                    , Process.sleep 50
                        |> Task.perform (\_ -> HeartbeatVolDebounce id vol)
                    )

                Nothing ->
                    ( model, Cmd.none )

        HeartbeatVolDebounce id vol ->
            if id == model.nextDebounce - 1 || True then
                ( model
                , Ports.websocketSend (encodeSetHeartbeatVolume vol)
                )

            else
                ( model, Cmd.none )

        SetBoopCount valStr ->
            case String.toInt valStr of
                Just count ->
                    let
                        id =
                            model.nextDebounce
                    in
                    ( { model
                        | boopCount = count
                        , nextDebounce = id + 1
                      }
                    , Process.sleep 50
                        |> Task.perform (\_ -> BoopCountDebounce id count)
                    )

                Nothing ->
                    ( model, Cmd.none )

        BoopCountDebounce id count ->
            if id == model.nextDebounce - 1 then
                ( model
                , Ports.websocketSend (encodeSetBoopCount count)
                )

            else
                ( model, Cmd.none )

        SetDroneInterpCurve index valStr ->
            case String.toFloat valStr of
                Just curve ->
                    let
                        id =
                            model.nextDebounce
                    in
                    ( { model
                        | checks =
                            updateCheckByKindIndex "drone"
                                index
                                (\c -> { c | interpCurve = curve })
                                model.checks
                        , nextDebounce = id + 1
                      }
                    , Process.sleep 50
                        |> Task.perform
                            (\_ -> DroneInterpCurveDebounce id index curve)
                    )

                Nothing ->
                    ( model, Cmd.none )

        DroneInterpCurveDebounce _ index curve ->
            ( model
            , Ports.websocketSend (encodeSetDroneInterpCurve index curve)
            )

        OverrideCheck index severity ->
            if severity == "" then
                ( model
                , Ports.websocketSend
                    (encodeClearOverride "heartbeat" index)
                )

            else
                ( model
                , Ports.websocketSend
                    (encodeOverrideCheck "heartbeat"
                        index
                        (severityToMetric severity)
                    )
                )

        ClearCheckOverride index ->
            ( model
            , Ports.websocketSend (encodeClearOverride "heartbeat" index)
            )

        SetDroneBoops index valStr ->
            case String.toInt valStr of
                Just boops ->
                    ( model
                    , Ports.websocketSend
                        (Protocol.encodeSetDroneBoops index boops)
                    )

                Nothing ->
                    ( model, Cmd.none )

        OverrideDroneValue index valStr ->
            case String.toFloat valStr of
                Just val ->
                    ( { model
                        | checks =
                            updateCheckByKindIndex "drone"
                                index
                                (\c -> { c | value = val })
                                model.checks
                      }
                    , Ports.websocketSend
                        (encodeOverrideCheck "drone" index val)
                    )

                Nothing ->
                    ( model, Cmd.none )

        ClearDroneOverride index ->
            ( model
            , Ports.websocketSend (encodeClearOverride "drone" index)
            )

        ToggleHeartbeatLoop ->
            let
                newLoop =
                    not model.heartbeatLoop
            in
            ( { model | heartbeatLoop = newLoop }
            , Ports.websocketSend (encodeSetHeartbeatLoop newLoop)
            )

        TriggerHeartbeat ->
            ( model
            , Ports.websocketSend encodeTriggerHeartbeat
            )

        RevertAll ->
            ( model
            , Ports.websocketSend encodeRevertAll
            )

        Export ->
            ( model
            , Ports.websocketSend encodeExportPatch
            )

        DismissExport ->
            ( { model | exportData = Nothing, importError = Nothing }, Cmd.none )

        SetExportTab tab ->
            ( { model | exportTab = tab }, Cmd.none )

        ToggleLockParam layer maybeIndex param ->
            case ( layer, maybeIndex ) of
                ( "drone_lo", Just i ) ->
                    let
                        isLocked =
                            getCheckByKindIndex "drone" i model.checks
                                |> Maybe.map
                                    (\c -> List.member param c.lockedParamsLo)
                                |> Maybe.withDefault False
                    in
                    ( { model
                        | checks =
                            updateCheckByKindIndex "drone"
                                i
                                (\c ->
                                    if isLocked then
                                        { c
                                            | lockedParamsLo =
                                                List.filter (\p -> p /= param)
                                                    c.lockedParamsLo
                                        }

                                    else
                                        { c | lockedParamsLo = param :: c.lockedParamsLo }
                                )
                                model.checks
                      }
                    , Ports.websocketSend
                        (if isLocked then
                            encodeUnlockParam layer maybeIndex param

                         else
                            encodeLockParam layer maybeIndex param
                        )
                    )

                ( "drone_hi", Just i ) ->
                    let
                        isLocked =
                            getCheckByKindIndex "drone" i model.checks
                                |> Maybe.map
                                    (\c -> List.member param c.lockedParamsHi)
                                |> Maybe.withDefault False
                    in
                    ( { model
                        | checks =
                            updateCheckByKindIndex "drone"
                                i
                                (\c ->
                                    if isLocked then
                                        { c
                                            | lockedParamsHi =
                                                List.filter (\p -> p /= param)
                                                    c.lockedParamsHi
                                        }

                                    else
                                        { c | lockedParamsHi = param :: c.lockedParamsHi }
                                )
                                model.checks
                      }
                    , Ports.websocketSend
                        (if isLocked then
                            encodeUnlockParam layer maybeIndex param

                         else
                            encodeLockParam layer maybeIndex param
                        )
                    )

                _ ->
                    if Set.member param model.lockedParams then
                        ( { model
                            | lockedParams =
                                Set.remove param model.lockedParams
                          }
                        , Ports.websocketSend
                            (encodeUnlockParam layer maybeIndex param)
                        )

                    else
                        ( { model
                            | lockedParams =
                                Set.insert param model.lockedParams
                          }
                        , Ports.websocketSend
                            (encodeLockParam layer maybeIndex param)
                        )

        ToggleLockDrone index ->
            if Set.member index model.lockedDrones then
                ( { model | lockedDrones = Set.remove index model.lockedDrones }
                , Ports.websocketSend (encodeUnlockDrone index)
                )

            else
                ( { model | lockedDrones = Set.insert index model.lockedDrones }
                , Ports.websocketSend (encodeLockDrone index)
                )

        UnlockAll ->
            ( { model | lockedParams = Set.empty, lockedDrones = Set.empty }
            , Ports.websocketSend encodeUnlockAll
            )

        SetBoopFreq index valStr ->
            case String.toFloat valStr of
                Just freq ->
                    let
                        key =
                            "boop_freq:" ++ String.fromInt index

                        id =
                            model.nextDebounce

                        newSpecs =
                            List.indexedMap
                                (\i s ->
                                    if i == index then
                                        { s | freq = freq, pinned = True }

                                    else
                                        s
                                )
                                model.boopSpecs
                    in
                    ( { model
                        | boopSpecs = newSpecs
                        , debounces = Dict.insert key id model.debounces
                        , nextDebounce = id + 1
                      }
                    , Process.sleep 50
                        |> Task.perform (\_ -> BoopFreqDebounce index id freq)
                    )

                Nothing ->
                    ( model, Cmd.none )

        BoopFreqDebounce index id freq ->
            let
                key =
                    "boop_freq:" ++ String.fromInt index
            in
            if Dict.get key model.debounces == Just id then
                ( model
                , Ports.websocketSend
                    (encodeSetBoopSpec index (Just freq) Nothing)
                )

            else
                ( model, Cmd.none )

        SetBoopDuration index valStr ->
            case String.toFloat valStr of
                Just dur ->
                    let
                        key =
                            "boop_dur:" ++ String.fromInt index

                        id =
                            model.nextDebounce

                        newSpecs =
                            List.indexedMap
                                (\i s ->
                                    if i == index then
                                        { s | duration = dur, pinned = True }

                                    else
                                        s
                                )
                                model.boopSpecs
                    in
                    ( { model
                        | boopSpecs = newSpecs
                        , debounces = Dict.insert key id model.debounces
                        , nextDebounce = id + 1
                      }
                    , Process.sleep 50
                        |> Task.perform (\_ -> BoopDurationDebounce index id dur)
                    )

                Nothing ->
                    ( model, Cmd.none )

        BoopDurationDebounce index id dur ->
            let
                key =
                    "boop_dur:" ++ String.fromInt index
            in
            if Dict.get key model.debounces == Just id then
                ( model
                , Ports.websocketSend
                    (encodeSetBoopSpec index Nothing (Just dur))
                )

            else
                ( model, Cmd.none )

        ClearBoopPin index ->
            ( model
            , Ports.websocketSend (encodeClearBoopPin index)
            )

        SetDroneNoteFreq droneIdx noteIdx valStr ->
            case String.toFloat valStr of
                Just freq ->
                    let
                        key =
                            "drone_note_freq:"
                                ++ String.fromInt droneIdx
                                ++ ":"
                                ++ String.fromInt noteIdx

                        id =
                            model.nextDebounce
                    in
                    ( { model
                        | checks =
                            updateCheckByKindIndex "drone"
                                droneIdx
                                (\c ->
                                    { c
                                        | specs =
                                            List.indexedMap
                                                (\ni s ->
                                                    if ni == noteIdx then
                                                        { s | freq = freq, pinned = True }

                                                    else
                                                        s
                                                )
                                                c.specs
                                    }
                                )
                                model.checks
                        , debounces = Dict.insert key id model.debounces
                        , nextDebounce = id + 1
                      }
                    , Process.sleep 50
                        |> Task.perform
                            (\_ -> DroneNoteFreqDebounce droneIdx noteIdx id freq)
                    )

                Nothing ->
                    ( model, Cmd.none )

        DroneNoteFreqDebounce droneIdx noteIdx id freq ->
            let
                key =
                    "drone_note_freq:"
                        ++ String.fromInt droneIdx
                        ++ ":"
                        ++ String.fromInt noteIdx
            in
            if Dict.get key model.debounces == Just id then
                ( model
                , Ports.websocketSend
                    (Protocol.encodeSetDroneSpec droneIdx noteIdx (Just freq) Nothing)
                )

            else
                ( model, Cmd.none )

        SetDroneNoteDuration droneIdx noteIdx valStr ->
            case String.toFloat valStr of
                Just dur ->
                    let
                        key =
                            "drone_note_dur:"
                                ++ String.fromInt droneIdx
                                ++ ":"
                                ++ String.fromInt noteIdx

                        id =
                            model.nextDebounce
                    in
                    ( { model
                        | checks =
                            updateCheckByKindIndex "drone"
                                droneIdx
                                (\c ->
                                    { c
                                        | specs =
                                            List.indexedMap
                                                (\ni s ->
                                                    if ni == noteIdx then
                                                        { s | duration = dur, pinned = True }

                                                    else
                                                        s
                                                )
                                                c.specs
                                    }
                                )
                                model.checks
                        , debounces = Dict.insert key id model.debounces
                        , nextDebounce = id + 1
                      }
                    , Process.sleep 50
                        |> Task.perform
                            (\_ -> DroneNoteDurationDebounce droneIdx noteIdx id dur)
                    )

                Nothing ->
                    ( model, Cmd.none )

        DroneNoteDurationDebounce droneIdx noteIdx id dur ->
            let
                key =
                    "drone_note_dur:"
                        ++ String.fromInt droneIdx
                        ++ ":"
                        ++ String.fromInt noteIdx
            in
            if Dict.get key model.debounces == Just id then
                ( model
                , Ports.websocketSend
                    (Protocol.encodeSetDroneSpec droneIdx noteIdx Nothing (Just dur))
                )

            else
                ( model, Cmd.none )

        ClearDronePin droneIdx noteIdx ->
            ( model
            , Ports.websocketSend (Protocol.encodeClearDronePin droneIdx noteIdx)
            )

        SetImportText text ->
            ( { model | importText = text, importError = Nothing }, Cmd.none )

        SubmitImport ->
            ( { model | importError = Nothing }
            , Ports.websocketSend (encodeImportConfig model.importText)
            )

        NoOp ->
            ( model, Cmd.none )


handleServerMsg : String -> Model -> ( Model, Cmd Msg )
handleServerMsg raw model =
    case decodeServerMsg raw of
        Just (StateMsg s) ->
            ( { model
                | patch = s.patch
                , muted = s.muted
                , masterVolume = s.masterVolume
                , heartbeatVolume = s.heartbeatVolume
                , heartbeatLoop = s.heartbeatLoop
                , boopCount = s.boopCount
                , checks = s.checks
                , lockedParams = Set.fromList s.lockedParams
                , lockedDrones = Set.fromList s.lockedDrones
                , boopSpecs = s.boopSpecs
                , boopSpecRanges = s.boopSpecRanges
                , connected = True
              }
            , Cmd.none
            )

        Just (ParamChanged layer maybeIndex param value) ->
            let
                updateParam p =
                    if p.name == param then
                        { p | value = value }

                    else
                        p
            in
            case ( layer, maybeIndex ) of
                ( "drone_lo", Just i ) ->
                    ( { model
                        | checks =
                            updateCheckByKindIndex "drone"
                                i
                                (\c -> { c | patchLo = List.map updateParam c.patchLo })
                                model.checks
                      }
                    , Cmd.none
                    )

                ( "drone_hi", Just i ) ->
                    ( { model
                        | checks =
                            updateCheckByKindIndex "drone"
                                i
                                (\c -> { c | patchHi = List.map updateParam c.patchHi })
                                model.checks
                      }
                    , Cmd.none
                    )

                _ ->
                    ( { model | patch = List.map updateParam model.patch }
                    , Cmd.none
                    )

        Just (MuteChanged muted) ->
            ( { model | muted = muted }, Cmd.none )

        Just (VolumeChanged layer maybeIndex vol) ->
            case layer of
                "master" ->
                    ( { model | masterVolume = vol }, Cmd.none )

                "heartbeat" ->
                    ( { model | heartbeatVolume = vol }, Cmd.none )

                _ ->
                    ( model, Cmd.none )

        Just (OverrideChanged layer index maybeValue overridden) ->
            ( { model
                | checks =
                    updateCheckByKindIndex layer
                        index
                        (\c ->
                            { c
                                | value =
                                    maybeValue
                                        |> Maybe.andThen String.toFloat
                                        |> Maybe.withDefault c.value
                                , overridden = overridden
                            }
                        )
                        model.checks
              }
            , Cmd.none
            )

        Just (DroneConfigChanged index boops) ->
            ( { model
                | checks =
                    updateCheckByKindIndex "drone"
                        index
                        (\c -> { c | boops = boops })
                        model.checks
              }
            , Cmd.none
            )

        Just (DroneInterpCurveChanged index curve) ->
            ( { model
                | checks =
                    updateCheckByKindIndex "drone"
                        index
                        (\c -> { c | interpCurve = curve })
                        model.checks
              }
            , Cmd.none
            )

        Just (BoopCountChanged count) ->
            ( { model | boopCount = count }, Cmd.none )

        Just (HeartbeatLoopChanged enabled) ->
            ( { model | heartbeatLoop = enabled }, Cmd.none )

        Just (CheckLog entry) ->
            ( { model | checkLog = entry :: List.take 99 model.checkLog }
            , Cmd.none
            )

        Just (PatchExport data) ->
            ( { model | exportData = Just data }, Cmd.none )

        Just (LockedParamsChanged layer maybeIndex params) ->
            case ( layer, maybeIndex ) of
                ( "drone_lo", Just i ) ->
                    ( { model
                        | checks =
                            updateCheckByKindIndex "drone"
                                i
                                (\c -> { c | lockedParamsLo = params })
                                model.checks
                      }
                    , Cmd.none
                    )

                ( "drone_hi", Just i ) ->
                    ( { model
                        | checks =
                            updateCheckByKindIndex "drone"
                                i
                                (\c -> { c | lockedParamsHi = params })
                                model.checks
                      }
                    , Cmd.none
                    )

                _ ->
                    ( { model | lockedParams = Set.fromList params }
                    , Cmd.none
                    )

        Just (LockedDronesChanged indices) ->
            ( { model | lockedDrones = Set.fromList indices }, Cmd.none )

        Just (BoopSpecsChanged specs) ->
            ( { model | boopSpecs = specs }, Cmd.none )

        Just (DroneSpecsChanged index newSpecs) ->
            ( { model
                | checks =
                    updateCheckByKindIndex "drone"
                        index
                        (\c -> { c | specs = newSpecs })
                        model.checks
              }
            , Cmd.none
            )

        Just (ImportError message) ->
            ( { model | importError = Just message }, Cmd.none )

        Just Connected ->
            ( { model | connected = True }, Cmd.none )

        Just Disconnected ->
            ( { model | connected = False }, Cmd.none )

        Nothing ->
            ( model, Cmd.none )



-- VIEW


view : Model -> Browser.Document Msg
view model =
    { title = "sonify-health"
    , body =
        [ div [ class "app" ]
            [ viewNavbar
            , viewToolbar model
            , viewPage model
            , viewExportModal model
            ]
        ]
    }


viewNavbar : Html Msg
viewNavbar =
    nav [ class "navbar" ]
        [ a [ href "/", class "nav-link" ] [ text "Home" ]
        , a [ href "/me", class "nav-link" ] [ text "Me" ]
        , a [ href "/scalar", class "nav-link" ] [ text "API Docs" ]
        ]


viewPage : Model -> Html Msg
viewPage model =
    case model.route of
        Home ->
            div [ class "panels" ]
                [ viewHeartbeatPanel model
                , viewDronePanel model
                , viewCheckLog model
                ]

        Me ->
            viewMePage model.me

        NotFound ->
            div [ class "panel" ]
                [ h2 [ class "panel-heading" ] [ text "Not Found" ]
                , p [ class "text-muted" ]
                    [ text "The page you requested does not exist." ]
                ]


viewMePage : MeStatus -> Html Msg
viewMePage status =
    div [ class "panel" ]
        [ h2 [ class "panel-heading" ] [ text "Me" ]
        , case status of
            MeLoading ->
                p [ class "text-muted" ] [ text "Loading..." ]

            MeFailed ->
                p [ class "text-muted" ]
                    [ text "Failed to load user information." ]

            MeLoaded info ->
                div []
                    [ p []
                        [ text ("Name: " ++ info.name) ]
                    , p []
                        [ text
                            ("Authentication: "
                                ++ (if info.authEnabled then
                                        "enabled"

                                    else
                                        "disabled"
                                   )
                            )
                        ]
                    ]
        ]


viewToolbar : Model -> Html Msg
viewToolbar model =
    header [ class "toolbar" ]
        [ h1 [ class "toolbar-title" ] [ text "sonify-health" ]
        , div [ class "toolbar-actions" ]
            [ span
                [ class
                    (if model.connected then
                        "status-dot-connected"

                     else
                        "status-dot-disconnected"
                    )
                ]
                []
            , button
                [ class
                    (if model.muted then
                        "btn-mute"

                     else
                        "btn-action"
                    )
                , onClick ToggleMute
                ]
                [ text
                    (if model.muted then
                        "Unmute"

                     else
                        "Mute"
                    )
                ]
            , label [ class "slider-label" ] [ text "Master" ]
            , input
                [ type_ "range"
                , Html.Attributes.min "0"
                , Html.Attributes.max "1"
                , step "0.01"
                , value (String.fromFloat model.masterVolume)
                , onInput SetMasterVolume
                , class "slider"
                ]
                []
            , input
                [ type_ "number"
                , Html.Attributes.min "0"
                , Html.Attributes.max "1"
                , step "0.01"
                , value (String.fromFloat model.masterVolume)
                , onInput SetMasterVolume
                , class "slider-value-input"
                ]
                []
            , button [ class "btn-action", onClick RevertAll ]
                [ text "Revert" ]
            , button [ class "btn-action", onClick UnlockAll ]
                [ text "Unlock All" ]
            , button [ class "btn-action", onClick Export ]
                [ text "Export" ]
            ]
        ]


viewPatchSlider : String -> Maybe Int -> Set String -> PatchParam -> Html Msg
viewPatchSlider layer maybeIndex locked param =
    let
        isLocked =
            Set.member param.name locked
    in
    div [ class "slider-row" ]
        [ button
            [ class
                (if isLocked then
                    "btn-lock-active"

                 else
                    "btn-lock"
                )
            , onClick (ToggleLockParam layer maybeIndex param.name)
            ]
            [ text
                (if isLocked then
                    "L"

                 else
                    "U"
                )
            ]
        , label [ class "slider-label", title param.description ]
            [ text (formatParamName param.name) ]
        , input
            [ type_ "range"
            , Html.Attributes.min (String.fromFloat param.min)
            , Html.Attributes.max (String.fromFloat param.max)
            , step (String.fromFloat param.step)
            , value (String.fromFloat param.value)
            , onInput (SetPatchParam layer maybeIndex param.name)
            , class "slider"
            ]
            []
        , input
            [ type_ "number"
            , Html.Attributes.min (String.fromFloat param.min)
            , Html.Attributes.max (String.fromFloat param.max)
            , step (String.fromFloat param.step)
            , value (String.fromFloat param.value)
            , onInput (SetPatchParam layer maybeIndex param.name)
            , class "slider-value-input"
            ]
            []
        ]


viewHeartbeatPanel : Model -> Html Msg
viewHeartbeatPanel model =
    let
        hbChecks =
            List.filter (\c -> c.kind == "heartbeat") model.checks
    in
    section [ class "panel" ]
        [ h2 [ class "panel-heading" ] [ text "Heartbeat" ]
        , div [ class "control-row" ]
            [ label [ class "slider-label" ] [ text "Volume" ]
            , input
                [ type_ "range"
                , Html.Attributes.min "0"
                , Html.Attributes.max "1"
                , step "0.01"
                , value (String.fromFloat model.heartbeatVolume)
                , onInput SetHeartbeatVolume
                , class "slider"
                ]
                []
            , input
                [ type_ "number"
                , Html.Attributes.min "0"
                , Html.Attributes.max "1"
                , step "0.01"
                , value (String.fromFloat model.heartbeatVolume)
                , onInput SetHeartbeatVolume
                , class "slider-value-input"
                ]
                []
            ]
        , div [ class "control-row" ]
            [ label [ class "slider-label" ] [ text "Boops" ]
            , input
                [ type_ "range"
                , Html.Attributes.min "1"
                , Html.Attributes.max "8"
                , step "1"
                , value (String.fromInt model.boopCount)
                , onInput SetBoopCount
                , class "slider"
                ]
                []
            , span [ class "slider-value" ]
                [ text (String.fromInt model.boopCount) ]
            ]
        , div [ class "control-row" ]
            [ label [ class "toggle-label" ]
                [ input
                    [ type_ "checkbox"
                    , checked model.heartbeatLoop
                    , onClick ToggleHeartbeatLoop
                    ]
                    []
                , text " Loop"
                ]
            , button
                [ class "btn-trigger"
                , onClick TriggerHeartbeat
                ]
                [ text "Play Now" ]
            ]
        , viewBoopSpecs model.boopCount hbChecks model.boopSpecs model.boopSpecRanges
        , if List.isEmpty hbChecks then
            text ""

          else
            div [ class "checks-list" ]
                (h3 [ class "panel-subheading" ] [ text "Checks" ]
                    :: List.map viewCheck hbChecks
                )
        , if List.isEmpty model.patch then
            text ""

          else
            div [ class "slider-grid" ]
                (h3 [ class "panel-subheading" ] [ text "Patch" ]
                    :: List.map
                        (viewPatchSlider "heartbeat" Nothing model.lockedParams)
                        model.patch
                )
        ]


viewBoopSpecs : Int -> List CheckInfo -> List BoopSpecInfo -> BoopSpecRanges -> Html Msg
viewBoopSpecs boopCount checks specs ranges =
    if List.isEmpty specs then
        text ""

    else
        div [ class "boop-specs" ]
            (h3 [ class "panel-subheading" ] [ text "Boop Specs" ]
                :: List.indexedMap (viewBoopRow boopCount checks ranges) specs
            )


viewBoopRow : Int -> List CheckInfo -> BoopSpecRanges -> Int -> BoopSpecInfo -> Html Msg
viewBoopRow boopCount checks ranges index spec =
    let
        checkIdx =
            if boopCount > 0 then
                index // boopCount

            else
                0

        checkName =
            List.drop checkIdx checks
                |> List.head
                |> Maybe.map .name
                |> Maybe.withDefault "?"
    in
    div [ class "boop-row" ]
        [ span [ class "boop-index" ]
            [ text (String.fromInt index) ]
        , span [ class "boop-check-label" ]
            [ text checkName ]
        , label [ class "slider-label" ] [ text "Freq" ]
        , input
            [ type_ "range"
            , Html.Attributes.min (String.fromFloat ranges.freqMin)
            , Html.Attributes.max (String.fromFloat ranges.freqMax)
            , step (String.fromFloat ranges.freqStep)
            , value (String.fromFloat spec.freq)
            , onInput (SetBoopFreq index)
            , class "slider"
            ]
            []
        , input
            [ type_ "number"
            , Html.Attributes.min (String.fromFloat ranges.freqMin)
            , Html.Attributes.max (String.fromFloat ranges.freqMax)
            , step (String.fromFloat ranges.freqStep)
            , value (String.fromFloat spec.freq)
            , onInput (SetBoopFreq index)
            , class "slider-value-input"
            ]
            []
        , label [ class "slider-label" ] [ text "Dur" ]
        , input
            [ type_ "range"
            , Html.Attributes.min (String.fromFloat ranges.durationMin)
            , Html.Attributes.max (String.fromFloat ranges.durationMax)
            , step (String.fromFloat ranges.durationStep)
            , value (String.fromFloat spec.duration)
            , onInput (SetBoopDuration index)
            , class "slider"
            ]
            []
        , input
            [ type_ "number"
            , Html.Attributes.min (String.fromFloat ranges.durationMin)
            , Html.Attributes.max (String.fromFloat ranges.durationMax)
            , step (String.fromFloat ranges.durationStep)
            , value (String.fromFloat spec.duration)
            , onInput (SetBoopDuration index)
            , class "slider-value-input"
            ]
            []
        , if spec.pinned then
            button
                [ class "btn-live"
                , onClick (ClearBoopPin index)
                ]
                [ text "Unpin" ]

          else
            text ""
        ]


viewCheck : CheckInfo -> Html Msg
viewCheck check =
    let
        severity =
            metricToSeverity check.value
    in
    div [ class "check-row" ]
        [ span [ class "check-name" ] [ text check.name ]
        , span [ class ("badge-" ++ severity) ]
            [ text severity ]
        , select
            [ onInput (OverrideCheck check.checkIndex)
            , class "override-select"
            ]
            [ option
                [ value ""
                , selected (not check.overridden)
                ]
                [ text "live" ]
            , option
                [ value "healthy"
                , selected (check.overridden && severity == "healthy")
                ]
                [ text "healthy" ]
            , option
                [ value "degraded"
                , selected (check.overridden && severity == "degraded")
                ]
                [ text "degraded" ]
            , option
                [ value "down"
                , selected (check.overridden && severity == "down")
                ]
                [ text "down" ]
            ]
        , if check.overridden then
            span [ class "override-indicator" ] [ text "(override)" ]

          else
            text ""
        ]


viewDronePanel : Model -> Html Msg
viewDronePanel model =
    let
        dChecks =
            List.filter (\c -> c.kind == "drone") model.checks
    in
    section [ class "panel" ]
        [ h2 [ class "panel-heading" ] [ text "Drones" ]
        , if List.isEmpty dChecks then
            p [ class "text-muted" ] [ text "No drone metrics configured." ]

          else
            div [ class "drone-list" ]
                (List.map (viewDrone model.lockedDrones) dChecks)
        ]


viewDrone : Set Int -> CheckInfo -> Html Msg
viewDrone lockedDrones check =
    let
        index =
            check.checkIndex

        isLocked =
            Set.member index lockedDrones
    in
    div [ class "drone-row" ]
        [ div [ class "drone-header" ]
            [ button
                [ class
                    (if isLocked then
                        "btn-lock-active"

                     else
                        "btn-lock"
                    )
                , onClick (ToggleLockDrone index)
                ]
                [ text
                    (if isLocked then
                        "L"

                     else
                        "U"
                    )
                ]
            , span [ class "drone-name" ] [ text check.name ]
            , label [ class "slider-label" ] [ text "Boops" ]
            , select
                [ onInput (SetDroneBoops index)
                , class "override-select"
                ]
                (List.map
                    (\n ->
                        option
                            [ value (String.fromInt n), selected (check.boops == n) ]
                            [ text (String.fromInt n) ]
                    )
                    (List.range 1 8)
                )
            ]
        , div [ class "control-row" ]
            [ label [ class "slider-label" ] [ text "Interp" ]
            , input
                [ type_ "range"
                , Html.Attributes.min "0.1"
                , Html.Attributes.max "5"
                , step "0.1"
                , value (String.fromFloat check.interpCurve)
                , onInput (SetDroneInterpCurve index)
                , class "slider"
                ]
                []
            , input
                [ type_ "number"
                , Html.Attributes.min "0.1"
                , Html.Attributes.max "5"
                , step "0.1"
                , value (String.fromFloat check.interpCurve)
                , onInput (SetDroneInterpCurve index)
                , class "slider-value-input"
                ]
                []
            ]
        , div [ class "control-row" ]
            [ label [ class "slider-label" ] [ text "Value" ]
            , span [ class "slider-value" ]
                [ text (formatFloat check.value) ]
            , if check.overridden then
                button
                    [ class "btn-live"
                    , onClick (ClearDroneOverride index)
                    ]
                    [ text "Live" ]

              else
                text ""
            ]
        , div [ class "control-row" ]
            [ label [ class "slider-label" ] [ text "Override" ]
            , input
                [ type_ "range"
                , Html.Attributes.min "0"
                , Html.Attributes.max "1"
                , step "0.01"
                , value (String.fromFloat check.value)
                , onInput (OverrideDroneValue index)
                , class "slider"
                ]
                []
            ]
        , viewDroneBoopSpecs index check.specs check.specRanges
        , h3 [ class "panel-subheading" ] [ text "Patch Lo" ]
        , div [ class "slider-grid" ]
            (List.map
                (viewPatchSlider "drone_lo"
                    (Just index)
                    (Set.fromList check.lockedParamsLo)
                )
                check.patchLo
            )
        , h3 [ class "panel-subheading" ] [ text "Patch Hi" ]
        , div [ class "slider-grid" ]
            (List.map
                (viewPatchSlider "drone_hi"
                    (Just index)
                    (Set.fromList check.lockedParamsHi)
                )
                check.patchHi
            )
        ]


viewDroneBoopSpecs : Int -> List BoopSpecInfo -> BoopSpecRanges -> Html Msg
viewDroneBoopSpecs droneIndex specs ranges =
    if List.isEmpty specs then
        text ""

    else
        div [ class "boop-specs" ]
            (h3 [ class "panel-subheading" ] [ text "Notes" ]
                :: List.indexedMap (viewDroneBoopRow droneIndex ranges) specs
            )


viewDroneBoopRow : Int -> BoopSpecRanges -> Int -> BoopSpecInfo -> Html Msg
viewDroneBoopRow droneIndex ranges noteIndex spec =
    div [ class "boop-row" ]
        [ span [ class "boop-index" ]
            [ text (String.fromInt noteIndex) ]
        , label [ class "slider-label" ] [ text "Freq" ]
        , input
            [ type_ "range"
            , Html.Attributes.min (String.fromFloat ranges.freqMin)
            , Html.Attributes.max (String.fromFloat ranges.freqMax)
            , step (String.fromFloat ranges.freqStep)
            , value (String.fromFloat spec.freq)
            , onInput (SetDroneNoteFreq droneIndex noteIndex)
            , class "slider"
            ]
            []
        , input
            [ type_ "number"
            , Html.Attributes.min (String.fromFloat ranges.freqMin)
            , Html.Attributes.max (String.fromFloat ranges.freqMax)
            , step (String.fromFloat ranges.freqStep)
            , value (String.fromFloat spec.freq)
            , onInput (SetDroneNoteFreq droneIndex noteIndex)
            , class "slider-value-input"
            ]
            []
        , label [ class "slider-label" ] [ text "Dur" ]
        , input
            [ type_ "range"
            , Html.Attributes.min (String.fromFloat ranges.durationMin)
            , Html.Attributes.max (String.fromFloat ranges.durationMax)
            , step (String.fromFloat ranges.durationStep)
            , value (String.fromFloat spec.duration)
            , onInput (SetDroneNoteDuration droneIndex noteIndex)
            , class "slider"
            ]
            []
        , input
            [ type_ "number"
            , Html.Attributes.min (String.fromFloat ranges.durationMin)
            , Html.Attributes.max (String.fromFloat ranges.durationMax)
            , step (String.fromFloat ranges.durationStep)
            , value (String.fromFloat spec.duration)
            , onInput (SetDroneNoteDuration droneIndex noteIndex)
            , class "slider-value-input"
            ]
            []
        , if spec.pinned then
            button
                [ class "btn-live"
                , onClick (ClearDronePin droneIndex noteIndex)
                ]
                [ text "Unpin" ]

          else
            text ""
        ]


viewCheckLog : Model -> Html Msg
viewCheckLog model =
    section [ class "panel-log" ]
        [ h2 [ class "panel-heading" ] [ text "Check Log" ]
        , if List.isEmpty model.checkLog then
            p [ class "text-muted" ] [ text "Waiting for check results..." ]

          else
            div [ class "log-entries" ]
                (List.map viewLogEntry model.checkLog)
        ]


viewLogEntry : CheckLogEntry -> Html Msg
viewLogEntry entry =
    div [ class "log-entry" ]
        [ span [ class "log-layer" ] [ text entry.layer ]
        , span [ class "log-name" ] [ text entry.name ]
        , span [ class ("badge-" ++ entry.result) ]
            [ text entry.result ]
        , if entry.overridden then
            span [ class "override-indicator" ] [ text "(override)" ]

          else
            text ""
        ]


viewExportModal : Model -> Html Msg
viewExportModal model =
    case model.exportData of
        Just data ->
            let
                tabButton label_ tabKey =
                    button
                        [ class
                            (if model.exportTab == tabKey then
                                "tab-active"

                             else
                                "tab"
                            )
                        , onClick (SetExportTab tabKey)
                        ]
                        [ text label_ ]

                isImport =
                    model.exportTab == "import"

                modalBody =
                    if isImport then
                        div []
                            [ textarea
                                [ class "export-textarea"
                                , placeholder "Paste TOML or JSON config here..."
                                , value model.importText
                                , onInput SetImportText
                                ]
                                []
                            , case model.importError of
                                Just err ->
                                    p [ class "import-error" ] [ text err ]

                                Nothing ->
                                    text ""
                            , button
                                [ class "btn-action"
                                , onClick SubmitImport
                                ]
                                [ text "Apply" ]
                            ]

                    else
                        let
                            content =
                                case model.exportTab of
                                    "json" ->
                                        data.json

                                    "nix" ->
                                        data.nix

                                    _ ->
                                        data.toml
                        in
                        div []
                            [ textarea
                                [ class "export-textarea"
                                , Html.Attributes.readonly True
                                , value content
                                ]
                                []
                            ]
            in
            div [ class "modal-backdrop", onClick DismissExport ]
                [ div
                    [ class "modal"
                    , Html.Events.stopPropagationOn "click"
                        (Decode.succeed ( NoOp, True ))
                    ]
                    [ h3 [ class "modal-title" ]
                        [ text
                            (if isImport then
                                "Import Config"

                             else
                                "Export Patch"
                            )
                        ]
                    , div [ class "tab-bar" ]
                        [ tabButton "TOML" "toml"
                        , tabButton "JSON" "json"
                        , tabButton "Nix" "nix"
                        , tabButton "Import" "import"
                        ]
                    , modalBody
                    , button
                        [ class "btn-action"
                        , onClick DismissExport
                        ]
                        [ text "Close" ]
                    ]
                ]

        Nothing ->
            text ""



-- HELPERS


formatParamName : String -> String
formatParamName name =
    String.replace "_" " " name


patchDebounceKey : String -> Maybe Int -> String -> String
patchDebounceKey layer maybeIndex name =
    "patch:"
        ++ layer
        ++ ":"
        ++ (Maybe.map String.fromInt maybeIndex |> Maybe.withDefault "_")
        ++ ":"
        ++ name


formatFloat : Float -> String
formatFloat f =
    let
        s =
            String.fromFloat (toFloat (round (f * 100)) / 100)
    in
    if String.contains "." s then
        s

    else
        s ++ ".0"


updateCheckByKindIndex : String -> Int -> (CheckInfo -> CheckInfo) -> List CheckInfo -> List CheckInfo
updateCheckByKindIndex kind idx updater checks =
    List.map
        (\c ->
            if c.kind == kind && c.checkIndex == idx then
                updater c

            else
                c
        )
        checks


getCheckByKindIndex : String -> Int -> List CheckInfo -> Maybe CheckInfo
getCheckByKindIndex kind idx checks =
    List.filter (\c -> c.kind == kind && c.checkIndex == idx) checks
        |> List.head


metricToSeverity : Float -> String
metricToSeverity v =
    if v <= 0.25 then
        "healthy"

    else if v <= 0.75 then
        "degraded"

    else
        "down"


severityToMetric : String -> Float
severityToMetric severity =
    case severity of
        "healthy" ->
            0.0

        "degraded" ->
            0.5

        "down" ->
            1.0

        _ ->
            0.0
