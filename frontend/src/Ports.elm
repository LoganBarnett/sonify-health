port module Ports exposing (websocketReceive, websocketSend)


port websocketSend : String -> Cmd msg


port websocketReceive : (String -> msg) -> Sub msg
