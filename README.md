# Train Station Map
A interactive Map for UK Mainline station ticking.

# Running an instance
## Prequsites

You need access to the following data products from the [Rail Data Marketplace](https://raildata.org.uk):
    1. https://raildata.org.uk/dashboard/dataProduct/P-88ffe920-471c-4fd9-8e0d-95d5b9b7a257/overview
    2. https://raildata.org.uk/dashboard/dataProduct/P-49f7a182-c71b-45a2-b0f0-3b52c9a2968c/overview

You also need a server capable of running either a docker container or a nix derivation

## Configuration
This is configured through a couple of environment variables
| Variable | Description | Example |
| --- | --- | --- | --- |
| RUST_LOG | The Log Level of the rust webserver | info |
| AUTH_PASSWORD | The password you will use to "sign in" | Password1234 | 
| RDM_TOCS_API_KEY | The "Consumer Key" of the Knowledgebase TOCS data prodict | T9axxxxxxxxxxxxxxxxxxxxxx5Drt |
| RDM_STATIONS_API_KEY | The "Consumer Key" of the Knowledgebase Stations data prodict | T9axxxxxxxxxxxxxxxxxxxxxx5Drt |
| ADDR | The address + port to listen on, OPTIONAL, has a default | 0.0.0.0:3000 |

You also need to give the stations.db file and the tile_cache folder a persistent location as they store the state and cache the OSM tiles. This is obviously different per method and covered below

## Hosting

### Docker
This docker compose file would probably work as an example. It's untested as I don't use docker :p

```yaml
services:
  train-stations-map:
    build:
      context: .
      dockerfile: Dockerfile
    ports:
      - "3000:3000"
    dns:
      - 1.1.1.1
      - 8.8.8.8
    environment:
      RUST_LOG: info
      AUTH_PASSWORD: change-me
      RDM_TOCS_API_KEY: change-me
      RDM_STATIONS_API_KEY: change-me
      FRONTEND_DIST: /opt/train-stations-map/frontend/dist
    volumes:
      - app-data:/app
    restart: unless-stopped

volumes:
  app-data:
```

### NixOS
Just write a systemd service :p It's not hard