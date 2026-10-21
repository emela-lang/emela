#include <assert.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
#include "emela_gpio.h"
#include "emela_uart.h"
#include "emela_i2c.h"
#include "emela_spi.h"
#include "emela_clock.h"
#include "emela_console.h"


static unsigned seen[23];
static unsigned trace_count;
static int strict_sequence;
static int error_op = -1;
static int bad_length;
static int ready_value = 1;
static int console_nul;
static int pending;
static EmelaTask pending_task;
static int32_t pending_ms;
static unsigned checks;

static int32_t record(unsigned op) {
  assert(op < 23);
  if (strict_sequence) assert(op == trace_count);
  ++seen[op];
  ++trace_count;
  return (int)op == error_op ? 71 : 0;
}

size_t fixture_heap_live(void);
#define heap_live fixture_heap_live

_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  fprintf(stderr, "unexpected defect %d: %.*s\n", kind, length, message);
  abort();
}
void emela_board_init(void) {}
void emela_board_idle(void) {}
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t previous_state) { assert(previous_state == 0); }

void emela_gpio_configure(EmelaGpioPin pin, EmelaGpioMode mode, EmelaGpioPull pull) {
  assert(pin.id == 5 && mode == EMELA_GPIO_MODE_OUTPUT_PUSH_PULL && pull == EMELA_GPIO_PULL_NO_PULL);
  (void)record(0);
}
void emela_gpio_write(EmelaGpioPin pin, EmelaGpioPinState state) {
  assert(pin.id == 5 && state == EMELA_GPIO_PIN_STATE_HIGH);
  (void)record(1);
}
void emela_gpio_toggle(EmelaGpioPin pin) { assert(pin.id == 5); (void)record(2); }
void emela_gpio_enable_interrupt(EmelaGpioPin pin, EmelaGpioEdge edge) {
  assert(pin.id == 5 && edge == EMELA_GPIO_EDGE_BOTH);
  (void)record(3);
}
void emela_gpio_disable_interrupt(EmelaGpioPin pin) { assert(pin.id == 5); (void)record(4); }
EmelaGpioPinState emela_gpio_read(EmelaGpioPin pin) {
  assert(pin.id == 5);
  (void)record(5);
  return EMELA_GPIO_PIN_STATE_HIGH;
}

static void check_input(const uint16_t *data, uint16_t length, const uint16_t *expected, uint16_t count) {
  assert(length == count);
  for (uint16_t i = 0; i < count; ++i) assert(data[i] == expected[i]);
}

static int32_t transmit(unsigned op, const uint16_t *data, uint16_t length) {
  static const uint16_t full[] = {0, 255, 65535};
  if (strict_sequence) check_input(data, length, full, 3);
  else {
    assert(length == 0 || length == 2);
    if (length) { assert(data[0] == 0 && data[1] == UINT16_MAX); }
  }
  return record(op);
}

static int32_t receive(unsigned op, uint16_t base, int32_t size, uint16_t *out, uint16_t capacity, uint16_t *length) {
  assert(capacity == 4 && size >= 0 && size <= capacity);
  int32_t status = record(op);
  if (status != 0) {
    *length = UINT16_MAX;
    return status;
  }
  for (int32_t i = 0; i < size; ++i) out[i] = (uint16_t)(base + i);
  *length = bad_length == 1 ? (uint16_t)(capacity + 1)
    : bad_length == 2 ? (uint16_t)(size + 1) : (uint16_t)size;
  return 0;
}

int32_t emela_uart_configure(EmelaUartId id, EmelaUartConfig config) {
  assert(id == EMELA_UART_ID_UART2 && config.baud_rate == 115200);
  assert(config.data_bits == EMELA_UART_DATA_BITS_BITS8 && config.stop_bits == EMELA_UART_STOP_BITS_ONE);
  assert(config.parity == EMELA_UART_PARITY_NO_PARITY && config.direction == EMELA_UART_DIRECTION_TRANSMIT);
  assert(config.flow_control == EMELA_UART_FLOW_CONTROL_NO_FLOW_CONTROL && config.oversampling == EMELA_UART_OVERSAMPLING_BY16);
  return record(6);
}
int32_t emela_uart_transmit(EmelaUartId id, const uint16_t *data, uint16_t length, int64_t timeout) {
  assert(id == EMELA_UART_ID_UART2 && timeout == 9);
  return transmit(7, data, length);
}
int32_t emela_uart_receive(EmelaUartId id, int32_t size, int64_t timeout, uint16_t *out, uint16_t capacity, uint16_t *length) {
  assert(id == EMELA_UART_ID_UART2 && timeout == 9);
  return receive(8, 10, size, out, capacity, length);
}

int32_t emela_i2c_configure(EmelaI2cId id, EmelaI2cConfig config) {
  assert(id == EMELA_I2C_ID_I2C1 && config.clock_speed_hz == 100000);
  assert(config.addressing == EMELA_I2C_ADDRESSING_SEVEN_BIT && config.duty_cycle == EMELA_I2C_DUTY_CYCLE_STANDARD);
  return record(9);
}
int32_t emela_i2c_master_transmit(EmelaI2cId id, int32_t address, const uint16_t *data, uint16_t length, int64_t timeout) {
  assert(id == EMELA_I2C_ID_I2C1 && address == 64 && timeout == 9);
  return transmit(10, data, length);
}
int32_t emela_i2c_master_receive(EmelaI2cId id, int32_t address, int32_t size, int64_t timeout, uint16_t *out, uint16_t capacity, uint16_t *length) {
  assert(id == EMELA_I2C_ID_I2C1 && address == 64 && timeout == 9);
  return receive(11, 20, size, out, capacity, length);
}
int32_t emela_i2c_memory_write(EmelaI2cId id, int32_t address, int32_t memory, EmelaI2cMemoryAddressSize size, const uint16_t *data, uint16_t length, int64_t timeout) {
  assert(id == EMELA_I2C_ID_I2C1 && address == 64 && memory == 7 && size == EMELA_I2C_MEMORY_ADDRESS_SIZE_BITS16 && timeout == 9);
  if (strict_sequence) {
    static const uint16_t expected[] = {20, 21};
    check_input(data, length, expected, 2);
    return record(12);
  }
  return transmit(12, data, length);
}
int32_t emela_i2c_memory_read(EmelaI2cId id, int32_t address, int32_t memory, EmelaI2cMemoryAddressSize size, int32_t count, int64_t timeout, uint16_t *out, uint16_t capacity, uint16_t *length) {
  assert(id == EMELA_I2C_ID_I2C1 && address == 64 && memory == 7 && size == EMELA_I2C_MEMORY_ADDRESS_SIZE_BITS16 && timeout == 9);
  return receive(13, 30, count, out, capacity, length);
}
int32_t emela_i2c_is_device_ready(EmelaI2cId id, int32_t address, int32_t trials, int64_t timeout, int32_t *out) {
  assert(id == EMELA_I2C_ID_I2C1 && address == 64 && trials == 3 && timeout == 9);
  int32_t status = record(14);
  *out = ready_value;
  return status;
}

int32_t emela_spi_configure(EmelaSpiId id, EmelaSpiConfig config) {
  assert(id == EMELA_SPI_ID_SPI1 && config.mode == EMELA_SPI_MODE_MASTER);
  assert(config.direction == EMELA_SPI_DIRECTION_TWO_LINE && config.data_size == EMELA_SPI_DATA_SIZE_BITS16);
  assert(config.polarity == EMELA_SPI_CLOCK_POLARITY_IDLE_LOW && config.phase == EMELA_SPI_CLOCK_PHASE_FIRST_EDGE);
  assert(config.prescaler == EMELA_SPI_PRESCALER_DIV16 && config.bit_order == EMELA_SPI_BIT_ORDER_MOST_SIGNIFICANT_FIRST);
  return record(15);
}
int32_t emela_spi_transmit(EmelaSpiId id, const uint16_t *data, uint16_t length, int64_t timeout) {
  assert(id == EMELA_SPI_ID_SPI1 && timeout == 9);
  return transmit(16, data, length);
}
int32_t emela_spi_receive(EmelaSpiId id, int32_t size, int64_t timeout, uint16_t *out, uint16_t capacity, uint16_t *length) {
  assert(id == EMELA_SPI_ID_SPI1 && timeout == 9);
  return receive(17, 40, size, out, capacity, length);
}
int32_t emela_spi_transfer(EmelaSpiId id, const uint16_t *data, uint16_t count, int64_t timeout, uint16_t *out, uint16_t capacity, uint16_t *length) {
  assert(id == EMELA_SPI_ID_SPI1 && timeout == 9);
  if (strict_sequence) {
    static const uint16_t expected[] = {40, 41};
    check_input(data, count, expected, 2);
  } else {
    assert(count == 0 || (count == 2 && data[0] == 0 && data[1] == UINT16_MAX));
  }
  return receive(18, 50, count, out, capacity, length);
}

int64_t emela_board_clock_now(void) { (void)record(19); return INT64_C(123456); }
void emela_board_clock_sleep(EmelaTask task, int32_t milliseconds) {
  assert(!pending);
  pending_task = task;
  pending_ms = milliseconds;
  (void)record(22);
  if (milliseconds == 0) assert(emela_task_complete_unit(task));
  else pending = 1;
}
void emela_board_console_print_line(const char *data, uint16_t length) {
  if (console_nul) assert(length == 3 && memcmp(data, "a\0b", 3) == 0);
  else assert(length == 2 && memcmp(data, "ok", 2) == 0);
  (void)record(20);
}
void emela_board_console_print_error_line(const char *data, uint16_t length) {
  if (console_nul) assert(length == 3 && memcmp(data, "a\0b", 3) == 0);
  else assert(length == 5 && memcmp(data, "error", 5) == 0);
  (void)record(21);
}

static int32_t run_all(void) {
  int32_t result = INT32_MIN;
  EmelaFiber fiber = exercise_all(&result);
  (void)emela_scheduler_run(1000);
  assert(emela_fiber_done(fiber));
  emela_fiber_release(fiber);
  assert(heap_live() == 0);
  ++checks;
  return result;
}

static void bounds(void) {
  strict_sequence = 0;
  error_op = -1;
  for (int kind = 0; kind < 5; ++kind) {
    for (int arg = 0; arg < 5; ++arg) {
      unsigned before = trace_count;
      int32_t result = exercise_bounded(kind, arg);
      int rejected = arg < 2 || arg == 3;
      assert(result == (arg < 2 ? -1 : arg == 3 ? -2 : kind == 4 && arg == 2 ? 2 : 0));
      assert(trace_count == before + (rejected ? 0 : 1));
      assert(heap_live() == 0);
      ++checks;
    }
  }
  for (int kind = 5; kind < 9; ++kind) {
    const int sizes[] = {-1, 0, 2, 4, 5};
    for (unsigned i = 0; i < sizeof(sizes)/sizeof(sizes[0]); ++i) {
      int32_t size = sizes[i];
      unsigned before = trace_count;
      assert(exercise_bounded(kind, size) == (size < 0 ? -1 : size > 4 ? -2 : size));
      assert(trace_count == before + (size < 0 || size > 4 ? 0 : 1));
      assert(heap_live() == 0);
      ++checks;
    }
    for (int invalid = 1; invalid <= 2; ++invalid) {
      bad_length = invalid;
      assert(exercise_bounded(kind, 2) == -2);
      assert(heap_live() == 0);
      ++checks;
    }
    bad_length = 0;
  }
  for (int invalid = 1; invalid <= 2; ++invalid) {
    bad_length = invalid;
    assert(exercise_bounded(4, 2) == -2);
    assert(heap_live() == 0);
    ++checks;
  }
  bad_length = 0;
}

int main(void) {
  for (unsigned iteration = 0; iteration < 64; ++iteration) {
    strict_sequence = 1;
    trace_count = 0;
    assert(run_all() == 73 && trace_count == 22);
    for (int op = 6; op <= 18; ++op) {
      trace_count = 0;
      error_op = op;
      assert(run_all() == 71 && trace_count == (unsigned)op + 1);
    }
    error_op = -1;
    ready_value = 0;
    trace_count = 0;
    assert(run_all() == -1000 && trace_count == 22);
    ready_value = 1;
    bounds();
    console_nul = 1;
    exercise_logs((EmelaString){.data = "a\0b", .length = 3, .owner = NULL});
    console_nul = 0;
    assert(heap_live() == 0);
    const int32_t milliseconds[] = {9, 0, -1};
    for (unsigned i = 0; i < sizeof(milliseconds)/sizeof(milliseconds[0]); ++i) {
      int32_t ms = milliseconds[i];
      int32_t result = INT32_MIN;
      EmelaFiber fiber = exercise_sleep_start(ms, &result);
      (void)emela_scheduler_run(1);
      assert(pending_ms == (ms < 0 ? 0 : ms));
      assert(result == INT32_MIN && !emela_fiber_done(fiber));
      if (ms > 0) {
        assert(pending);
        pending = 0;
        assert(emela_task_complete_unit(pending_task));
      } else assert(!pending);
      assert(result == INT32_MIN);
      (void)emela_scheduler_run(1000);
      assert(result == 77 && emela_fiber_done(fiber));
      emela_fiber_release(fiber);
      assert(!emela_task_complete_unit(pending_task));
      assert(heap_live() == 0);
      ++checks;
    }
  }
  for (unsigned op = 0; op < 23; ++op) assert(seen[op] > 0);
  assert(checks > 0 && heap_live() == 0);
  return 0;
}
