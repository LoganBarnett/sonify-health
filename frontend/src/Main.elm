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
    , voice : List VoiceParam
    , muted : Bool
    , heartbeatVolume : Float
    , heartbeatLoop : Bool
    , boopCount : Int
    , checks : List CheckInfo
    , drones : List DroneInfo
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
    | SetVoiceParam String (Maybe Int) String String
    | VoiceDebounce String (Maybe Int) String Int Float
    | ToggleMute
    | SetHeartbeatVolume String
    | HeartbeatVolDebounce Int Float
    | SetBoopCount String
    | BoopCountDebounce Int Int
    | SetDroneVolume Int String
    | DroneVolDebounce Int Int Float
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
      , voice = []
      , muted = False
      , heartbeatVolume = 1.0
      , heartbeatLoop = False
      , boopCount = 1
      , checks = []
      , drones = []
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

        SetVoiceParam layer maybeIndex name valStr ->
            case String.toFloat valStr of
                Just value ->
                    let
                        key =
                            voiceDebounceKey layer maybeIndex name

                        id =
                            model.nextDebounce

                        updateParam p =
                            if p.name == name then
                                { p | value = value }

                            else
                                p
                    in
                    case ( layer, maybeIndex ) of
                        ( "drone", Just i ) ->
                            ( { model
                                | drones =
                                    List.indexedMap
                                        (\idx d ->
                                            if idx == i then
                                                { d | voice = List.map updateParam d.voice }

                                            else
                                                d
                                        )
                                        model.drones
                                , debounces = Dict.insert key id model.debounces
                                , nextDebounce = id + 1
                              }
                            , Process.sleep 50
                                |> Task.perform
                                    (\_ -> VoiceDebounce layer maybeIndex name id value)
                            )

                        _ ->
                            ( { model
                                | voice = List.map updateParam model.voice
                                , debounces = Dict.insert key id model.debounces
                                , nextDebounce = id + 1
                              }
                            , Process.sleep 50
                                |> Task.perform
                                    (\_ -> VoiceDebounce layer maybeIndex name id value)
                            )

                Nothing ->
                    ( model, Cmd.none )

        VoiceDebounce layer maybeIndex name id value ->
            let
                key =
                    voiceDebounceKey layer maybeIndex name
            in
            if Dict.get key model.debounces == Just id then
                ( model
                , Ports.websocketSend
                    (encodeSetVoiceParam layer maybeIndex name value)
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

        SetDroneVolume index valStr ->
            case String.toFloat valStr of
                Just vol ->
                    let
                        id =
                            model.nextDebounce

                        newDrones =
                            List.indexedMap
                                (\i d ->
                                    if i == index then
                                        { d | volume = vol }

                                    else
                                        d
                                )
                                model.drones
                    in
                    ( { model
                        | drones = newDrones
                        , nextDebounce = id + 1
                      }
                    , Process.sleep 50
                        |> Task.perform
                            (\_ -> DroneVolDebounce id index vol)
                    )

                Nothing ->
                    ( model, Cmd.none )

        DroneVolDebounce _ index vol ->
            ( model
            , Ports.websocketSend (encodeSetDroneVolume index vol)
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
                    (encodeOverrideCheck "heartbeat" index severity)
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
                        | drones =
                            List.indexedMap
                                (\i d ->
                                    if i == index then
                                        { d | value = val }

                                    else
                                        d
                                )
                                model.drones
                      }
                    , Ports.websocketSend
                        (Protocol.encodeOverrideDrone index val)
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
            , Ports.websocketSend encodeExportVoice
            )

        DismissExport ->
            ( { model | exportData = Nothing, importError = Nothing }, Cmd.none )

        SetExportTab tab ->
            ( { model | exportTab = tab }, Cmd.none )

        ToggleLockParam layer maybeIndex param ->
            case ( layer, maybeIndex ) of
                ( "drone", Just i ) ->
                    let
                        isLocked =
                            List.drop i model.drones
                                |> List.head
                                |> Maybe.map
                                    (\d -> List.member param d.lockedParams)
                                |> Maybe.withDefault False

                        toggleLocked d =
                            if isLocked then
                                { d
                                    | lockedParams =
                                        List.filter (\p -> p /= param)
                                            d.lockedParams
                                }

                            else
                                { d | lockedParams = param :: d.lockedParams }
                    in
                    ( { model
                        | drones =
                            List.indexedMap
                                (\idx d ->
                                    if idx == i then
                                        toggleLocked d

                                    else
                                        d
                                )
                                model.drones
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
                | voice = s.voice
                , muted = s.muted
                , heartbeatVolume = s.heartbeatVolume
                , heartbeatLoop = s.heartbeatLoop
                , boopCount = s.boopCount
                , checks = s.checks
                , drones = s.drones
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
                ( "drone", Just i ) ->
                    ( { model
                        | drones =
                            List.indexedMap
                                (\idx d ->
                                    if idx == i then
                                        { d | voice = List.map updateParam d.voice }

                                    else
                                        d
                                )
                                model.drones
                      }
                    , Cmd.none
                    )

                _ ->
                    ( { model | voice = List.map updateParam model.voice }
                    , Cmd.none
                    )

        Just (MuteChanged muted) ->
            ( { model | muted = muted }, Cmd.none )

        Just (VolumeChanged layer maybeIndex vol) ->
            case layer of
                "heartbeat" ->
                    ( { model | heartbeatVolume = vol }, Cmd.none )

                "drone" ->
                    case maybeIndex of
                        Just index ->
                            ( { model
                                | drones =
                                    List.indexedMap
                                        (\i d ->
                                            if i == index then
                                                { d | volume = vol }

                                            else
                                                d
                                        )
                                        model.drones
                              }
                            , Cmd.none
                            )

                        Nothing ->
                            ( model, Cmd.none )

                _ ->
                    ( model, Cmd.none )

        Just (OverrideChanged layer index maybeValue overridden) ->
            case layer of
                "heartbeat" ->
                    ( { model
                        | checks =
                            List.indexedMap
                                (\i c ->
                                    if i == index then
                                        { c
                                            | severity =
                                                Maybe.withDefault
                                                    c.severity
                                                    maybeValue
                                            , overridden = overridden
                                        }

                                    else
                                        c
                                )
                                model.checks
                      }
                    , Cmd.none
                    )

                "drone" ->
                    ( { model
                        | drones =
                            List.indexedMap
                                (\i d ->
                                    if i == index then
                                        { d
                                            | overridden = overridden
                                            , value =
                                                maybeValue
                                                    |> Maybe.andThen String.toFloat
                                                    |> Maybe.withDefault d.value
                                        }

                                    else
                                        d
                                )
                                model.drones
                      }
                    , Cmd.none
                    )

                _ ->
                    ( model, Cmd.none )

        Just (DroneConfigChanged index baseFreq boops) ->
            ( { model
                | drones =
                    List.indexedMap
                        (\i d ->
                            if i == index then
                                { d | baseFreq = baseFreq, boops = boops }

                            else
                                d
                        )
                        model.drones
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

        Just (VoiceExport data) ->
            ( { model | exportData = Just data }, Cmd.none )

        Just (LockedParamsChanged layer maybeIndex params) ->
            case ( layer, maybeIndex ) of
                ( "drone", Just i ) ->
                    ( { model
                        | drones =
                            List.indexedMap
                                (\idx d ->
                                    if idx == i then
                                        { d | lockedParams = params }

                                    else
                                        d
                                )
                                model.drones
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
            , button [ class "btn-action", onClick RevertAll ]
                [ text "Revert" ]
            , button [ class "btn-action", onClick UnlockAll ]
                [ text "Unlock All" ]
            , button [ class "btn-action", onClick Export ]
                [ text "Export" ]
            ]
        ]


viewVoiceSlider : String -> Maybe Int -> Set String -> VoiceParam -> Html Msg
viewVoiceSlider layer maybeIndex locked param =
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
            , onInput (SetVoiceParam layer maybeIndex param.name)
            , class "slider"
            ]
            []
        , span [ class "slider-value" ]
            [ text (formatFloat param.value) ]
        ]


viewHeartbeatPanel : Model -> Html Msg
viewHeartbeatPanel model =
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
            , span [ class "slider-value" ]
                [ text (formatFloat model.heartbeatVolume) ]
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
        , viewBoopSpecs model.boopCount model.checks model.boopSpecs model.boopSpecRanges
        , if List.isEmpty model.checks then
            text ""

          else
            div [ class "checks-list" ]
                (h3 [ class "panel-subheading" ] [ text "Checks" ]
                    :: List.indexedMap viewCheck model.checks
                )
        , if List.isEmpty model.voice then
            text ""

          else
            div [ class "slider-grid" ]
                (h3 [ class "panel-subheading" ] [ text "Voice" ]
                    :: List.map
                        (viewVoiceSlider "heartbeat" Nothing model.lockedParams)
                        model.voice
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
        , span [ class "slider-value" ]
            [ text (formatFloat spec.freq) ]
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
        , span [ class "slider-value" ]
            [ text (formatFloat spec.duration) ]
        , if spec.pinned then
            button
                [ class "btn-live"
                , onClick (ClearBoopPin index)
                ]
                [ text "Unpin" ]

          else
            text ""
        ]


viewCheck : Int -> CheckInfo -> Html Msg
viewCheck index check =
    div [ class "check-row" ]
        [ span [ class "check-name" ] [ text check.name ]
        , span [ class ("badge-" ++ check.severity) ]
            [ text check.severity ]
        , select
            [ onInput (OverrideCheck index)
            , class "override-select"
            ]
            [ option
                [ value ""
                , selected (not check.overridden)
                ]
                [ text "live" ]
            , option
                [ value "healthy"
                , selected (check.overridden && check.severity == "healthy")
                ]
                [ text "healthy" ]
            , option
                [ value "degraded"
                , selected (check.overridden && check.severity == "degraded")
                ]
                [ text "degraded" ]
            , option
                [ value "down"
                , selected (check.overridden && check.severity == "down")
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
    section [ class "panel" ]
        [ h2 [ class "panel-heading" ] [ text "Drones" ]
        , if List.isEmpty model.drones then
            p [ class "text-muted" ] [ text "No drone metrics configured." ]

          else
            div [ class "drone-list" ]
                (List.indexedMap
                    (viewDrone model.lockedDrones)
                    model.drones
                )
        ]


viewDrone : Set Int -> Int -> DroneInfo -> Html Msg
viewDrone lockedDrones index drone =
    let
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
            , span [ class "drone-name" ] [ text drone.name ]
            , label [ class "slider-label" ] [ text "Boops" ]
            , select
                [ onInput (SetDroneBoops index)
                , class "override-select"
                ]
                (List.map
                    (\n ->
                        option
                            [ value (String.fromInt n), selected (drone.boops == n) ]
                            [ text (String.fromInt n) ]
                    )
                    (List.range 1 8)
                )
            ]
        , div [ class "control-row" ]
            [ label [ class "slider-label" ] [ text "Volume" ]
            , input
                [ type_ "range"
                , Html.Attributes.min "0"
                , Html.Attributes.max "1"
                , step "0.01"
                , value (String.fromFloat drone.volume)
                , onInput (SetDroneVolume index)
                , class "slider"
                ]
                []
            , span [ class "slider-value" ]
                [ text (formatFloat drone.volume) ]
            ]
        , div [ class "control-row" ]
            [ label [ class "slider-label" ] [ text "Value" ]
            , span [ class "slider-value" ]
                [ text (formatFloat drone.value) ]
            , if drone.overridden then
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
                , value (String.fromFloat drone.value)
                , onInput (OverrideDroneValue index)
                , class "slider"
                ]
                []
            ]
        , div [ class "slider-grid" ]
            (List.map
                (viewVoiceSlider "drone"
                    (Just index)
                    (Set.fromList drone.lockedParams)
                )
                drone.voice
            )
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
                                "Export Voice"
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


voiceDebounceKey : String -> Maybe Int -> String -> String
voiceDebounceKey layer maybeIndex name =
    "voice:"
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
