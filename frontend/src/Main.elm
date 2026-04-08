module Main exposing (main)

import Browser
import Dict exposing (Dict)
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (onClick, onInput)
import Json.Decode
import Ports
import Process
import Protocol exposing (..)
import Task


type alias Model =
    { connected : Bool
    , voice : List VoiceParam
    , muted : Bool
    , heartbeatVolume : Float
    , heartbeatLoop : Bool
    , checks : List CheckInfo
    , drones : List DroneInfo
    , checkLog : List CheckLogEntry
    , tomlExport : Maybe String
    , debounces : Dict String Int
    , nextDebounce : Int
    }


type Msg
    = WebSocketReceived String
    | SetVoiceParam String String
    | DebounceFired String Int Float
    | ToggleMute
    | SetHeartbeatVolume String
    | HeartbeatVolDebounce Int Float
    | SetDroneVolume Int String
    | DroneVolDebounce Int Int Float
    | OverrideCheck Int String
    | ClearCheckOverride Int
    | SetDroneTexture Int String
    | SetDroneRegister Int String
    | OverrideDroneValue Int String
    | ClearDroneOverride Int
    | ToggleHeartbeatLoop
    | TriggerHeartbeat
    | RevertAll
    | ExportToml
    | DismissExport
    | NoOp


main : Program () Model Msg
main =
    Browser.element
        { init = \_ -> init
        , view = view
        , update = update
        , subscriptions = subscriptions
        }


init : ( Model, Cmd Msg )
init =
    ( { connected = False
      , voice = []
      , muted = False
      , heartbeatVolume = 1.0
      , heartbeatLoop = False
      , checks = []
      , drones = []
      , checkLog = []
      , tomlExport = Nothing
      , debounces = Dict.empty
      , nextDebounce = 0
      }
    , Cmd.none
    )


subscriptions : Model -> Sub Msg
subscriptions _ =
    Ports.websocketReceive WebSocketReceived



-- UPDATE


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        WebSocketReceived raw ->
            handleServerMsg raw model

        SetVoiceParam name valStr ->
            case String.toFloat valStr of
                Just value ->
                    let
                        key =
                            "voice:" ++ name

                        id =
                            model.nextDebounce

                        newVoice =
                            List.map
                                (\p ->
                                    if p.name == name then
                                        { p | value = value }

                                    else
                                        p
                                )
                                model.voice
                    in
                    ( { model
                        | voice = newVoice
                        , debounces = Dict.insert key id model.debounces
                        , nextDebounce = id + 1
                      }
                    , Process.sleep 50
                        |> Task.perform (\_ -> DebounceFired key id value)
                    )

                Nothing ->
                    ( model, Cmd.none )

        DebounceFired key id value ->
            if Dict.get key model.debounces == Just id then
                let
                    paramName =
                        String.dropLeft 6 key
                in
                ( model
                , Ports.websocketSend
                    (encodeSetVoiceParam paramName value)
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

        SetDroneTexture index texture ->
            ( model
            , Ports.websocketSend
                (Protocol.encodeSetDroneTexture index texture)
            )

        SetDroneRegister index register ->
            ( model
            , Ports.websocketSend
                (Protocol.encodeSetDroneRegister index register)
            )

        OverrideDroneValue index valStr ->
            case String.toFloat valStr of
                Just val ->
                    ( model
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

        ExportToml ->
            ( model
            , Ports.websocketSend encodeExportToml
            )

        DismissExport ->
            ( { model | tomlExport = Nothing }, Cmd.none )

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
                , checks = s.checks
                , drones = s.drones
                , connected = True
              }
            , Cmd.none
            )

        Just (ParamChanged param value) ->
            ( { model
                | voice =
                    List.map
                        (\p ->
                            if p.name == param then
                                { p | value = value }

                            else
                                p
                        )
                        model.voice
              }
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
                                        { d | overridden = overridden }

                                    else
                                        d
                                )
                                model.drones
                      }
                    , Cmd.none
                    )

                _ ->
                    ( model, Cmd.none )

        Just (DroneConfigChanged index texture register) ->
            ( { model
                | drones =
                    List.indexedMap
                        (\i d ->
                            if i == index then
                                { d | texture = texture, register = register }

                            else
                                d
                        )
                        model.drones
              }
            , Cmd.none
            )

        Just (HeartbeatLoopChanged enabled) ->
            ( { model | heartbeatLoop = enabled }, Cmd.none )

        Just (CheckLog entry) ->
            ( { model | checkLog = entry :: List.take 99 model.checkLog }
            , Cmd.none
            )

        Just (TomlExport content) ->
            ( { model | tomlExport = Just content }, Cmd.none )

        Just Connected ->
            ( { model | connected = True }, Cmd.none )

        Just Disconnected ->
            ( { model | connected = False }, Cmd.none )

        Nothing ->
            ( model, Cmd.none )



-- VIEW


view : Model -> Html Msg
view model =
    div [ class "app" ]
        [ viewToolbar model
        , div [ class "panels" ]
            [ viewVoicePanel model
            , viewHeartbeatPanel model
            , viewDronePanel model
            , viewCheckLog model
            ]
        , viewExportModal model
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
            , button [ class "btn-action", onClick ExportToml ]
                [ text "Export TOML" ]
            , a [ href "/scalar", class "btn-action" ]
                [ text "API Docs" ]
            ]
        ]


viewVoicePanel : Model -> Html Msg
viewVoicePanel model =
    section [ class "panel" ]
        [ h2 [ class "panel-heading" ] [ text "Voice" ]
        , div [ class "slider-grid" ]
            (List.map viewVoiceSlider model.voice)
        ]


viewVoiceSlider : VoiceParam -> Html Msg
viewVoiceSlider param =
    div [ class "slider-row" ]
        [ label [ class "slider-label" ]
            [ text (formatParamName param.name) ]
        , input
            [ type_ "range"
            , Html.Attributes.min (String.fromFloat param.min)
            , Html.Attributes.max (String.fromFloat param.max)
            , step (String.fromFloat param.step)
            , value (String.fromFloat param.value)
            , onInput (SetVoiceParam param.name)
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
        , if List.isEmpty model.checks then
            text ""

          else
            div [ class "checks-list" ]
                (h3 [ class "panel-subheading" ] [ text "Checks" ]
                    :: List.indexedMap viewCheck model.checks
                )
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
                (List.indexedMap viewDrone model.drones)
        ]


viewDrone : Int -> DroneInfo -> Html Msg
viewDrone index drone =
    div [ class "drone-row" ]
        [ div [ class "drone-header" ]
            [ span [ class "drone-name" ] [ text drone.name ]
            , select
                [ onInput (SetDroneTexture index)
                , class "override-select"
                ]
                (List.map
                    (\t ->
                        option
                            [ value t, selected (drone.texture == t) ]
                            [ text t ]
                    )
                    [ "bong", "arpeggio", "thrum", "shimmer", "reactor", "warpcore" ]
                )
            , select
                [ onInput (SetDroneRegister index)
                , class "override-select"
                ]
                (List.map
                    (\r ->
                        option
                            [ value r, selected (drone.register == r) ]
                            [ text r ]
                    )
                    [ "low", "mid", "high" ]
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
    case model.tomlExport of
        Just content ->
            div [ class "modal-backdrop", onClick DismissExport ]
                [ div
                    [ class "modal"
                    , Html.Events.stopPropagationOn "click"
                        (Json.Decode.succeed ( NoOp, True ))
                    ]
                    [ h3 [ class "modal-title" ] [ text "Exported TOML" ]
                    , textarea
                        [ class "export-textarea"
                        , Html.Attributes.readonly True
                        , value content
                        ]
                        []
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
