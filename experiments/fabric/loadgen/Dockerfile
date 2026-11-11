FROM golang:1.27-bookworm AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY *.go ./
RUN CGO_ENABLED=0 go build -trimpath -o /loadgen .

FROM alpine:3.20
COPY --from=build /loadgen /loadgen
ENTRYPOINT ["/loadgen"]
