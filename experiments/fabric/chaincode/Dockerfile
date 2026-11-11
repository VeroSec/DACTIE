FROM golang:1.27-bookworm AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY *.go ./
RUN CGO_ENABLED=0 go build -trimpath -o /chaincode .

FROM alpine:3.20
COPY --from=build /chaincode /chaincode
USER 65534
ENTRYPOINT ["/chaincode"]
