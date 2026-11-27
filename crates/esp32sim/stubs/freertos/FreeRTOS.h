#pragma once
#include <stdint.h>
typedef uint32_t TickType_t;
typedef int BaseType_t;
typedef unsigned int UBaseType_t;
typedef void *TaskHandle_t;
typedef void *QueueHandle_t;
typedef void *SemaphoreHandle_t;
typedef void *TimerHandle_t;
typedef void (*TaskFunction_t)(void *);
#define pdTRUE 1
#define pdFALSE 0
#define pdPASS 1
#define pdFAIL 0
#define portMAX_DELAY 0xffffffffUL
#define portTICK_PERIOD_MS 1
#define configTICK_RATE_HZ 1000
#define pdMS_TO_TICKS(ms) ((TickType_t)(ms))
#define tskNO_AFFINITY 0x7FFFFFFF
#define portMUX_INITIALIZER_UNLOCKED {0}
typedef struct { uint32_t owner; } portMUX_TYPE;
#define portENTER_CRITICAL(m) ((void)(m))
#define portEXIT_CRITICAL(m) ((void)(m))
#define portENTER_CRITICAL_ISR(m) ((void)(m))
#define portEXIT_CRITICAL_ISR(m) ((void)(m))
#ifdef __cplusplus
extern "C" {
#endif
BaseType_t xTaskCreate(TaskFunction_t fn, const char *name, uint32_t stack, void *arg, UBaseType_t prio, TaskHandle_t *handle);
BaseType_t xTaskCreatePinnedToCore(TaskFunction_t fn, const char *name, uint32_t stack, void *arg, UBaseType_t prio, TaskHandle_t *handle, BaseType_t core);
void vTaskDelete(TaskHandle_t task);
void vTaskDelay(TickType_t ticks);
void vTaskDelayUntil(TickType_t *prev, TickType_t increment);
TickType_t xTaskGetTickCount(void);
BaseType_t xPortGetCoreID(void);
QueueHandle_t xQueueCreate(UBaseType_t length, UBaseType_t item_size);
BaseType_t xQueueSend(QueueHandle_t q, const void *item, TickType_t wait);
BaseType_t xQueueSendFromISR(QueueHandle_t q, const void *item, BaseType_t *woken);
BaseType_t xQueueReceive(QueueHandle_t q, void *buf, TickType_t wait);
SemaphoreHandle_t xSemaphoreCreateMutex(void);
SemaphoreHandle_t xSemaphoreCreateBinary(void);
BaseType_t xSemaphoreTake(SemaphoreHandle_t s, TickType_t wait);
BaseType_t xSemaphoreGive(SemaphoreHandle_t s);
BaseType_t xSemaphoreGiveFromISR(SemaphoreHandle_t s, BaseType_t *woken);
#ifdef __cplusplus
}
#endif
