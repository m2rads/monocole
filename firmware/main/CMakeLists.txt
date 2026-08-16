idf_component_register(SRCS "main.c" "gatt_svr.c" "tcp_server.c" "display.c"
                            "mic.c" "voice.c" "adpcm.c"
                    PRIV_REQUIRES bt nvs_flash esp_timer esp_lcd esp_driver_i2c
                                  esp_driver_i2s
                    INCLUDE_DIRS ".")
