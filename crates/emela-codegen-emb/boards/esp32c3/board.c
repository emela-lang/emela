#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include "emela_embedded_exports.h"
#include "emela_board.h"
#if __has_include("emela_gpio.h")
#include "emela_gpio.h"
#define BOARD_GPIO 1
#endif
#if __has_include("emela_uart.h")
#include "emela_uart.h"
#define BOARD_UART 1
#endif
#if __has_include("emela_clock.h")
#include "emela_clock.h"
#define BOARD_CLOCK 1
#endif

/* Register definitions follow the ESP-IDF v5.4.1 ESP32-C3 SoC headers. */
#define REG(address) (*(volatile uint32_t *)(uintptr_t)(address))
#define BIT(n) (UINT32_C(1) << (n))
#define UART_FIFO REG(0x60000000)
#define UART_STATUS REG(0x6000001c)
#define GPIO_OUT REG(0x60004004)
#define GPIO_OUT_SET REG(0x60004008)
#define GPIO_OUT_CLEAR REG(0x6000400c)
#define GPIO_ENABLE_SET REG(0x60004024)
#define GPIO_ENABLE_CLEAR REG(0x60004028)
#define GPIO_IN REG(0x6000403c)
#define GPIO_PIN(pin) REG(UINT32_C(0x60004074) + 4u * (pin))
#define GPIO_OUT_SELECT(pin) REG(UINT32_C(0x60004554) + 4u * (pin))
#define IO_MUX(pin) REG(UINT32_C(0x60009004) + 4u * (pin))
#define TIMER(offset) REG(UINT32_C(0x60023000) + (offset))
#define MATRIX(offset) REG(UINT32_C(0x600c2000) + (offset))
#define TIMER_BASE_CONFIG (BIT(31) | BIT(30))
#define TICKS_PER_MS UINT32_C(16000)
#define CPU_IRQ UINT32_C(1)

extern uint32_t _stack_guard_start[], _stack_bottom[], _stack_top[];
volatile uint32_t board_app_completed;
volatile int32_t board_app_result;
volatile uint32_t board_failures;
volatile uint32_t board_ticks;
volatile uint32_t board_notifications;
volatile uint32_t board_rejected_notifications;
volatile uint32_t board_uart_bytes;
volatile uint32_t board_stack_used;
volatile uint32_t board_stack_guard_ok;
volatile uint32_t board_trap_cause;
volatile uint32_t board_trap_pc;
static uint32_t configured_outputs;
#ifdef BOARD_GPIO
static uint32_t configured_inputs;
#endif
static unsigned event_locked;
static volatile unsigned in_trap;
#if defined(BOARD_CLOCK) && defined(EMELA_HAS_TASK_RUNTIME)
static volatile EmelaTask sleeping_task;
static volatile uint32_t remaining_ms;
#endif

void _init(void) {}
void _fini(void) {}

static uint32_t irq_disable(void) {
    uint32_t previous;
    __asm volatile("csrrci %0, mstatus, 8" : "=r"(previous) :: "memory");
    return previous & 8u;
}

static void irq_restore(uint32_t previous) {
    if (previous) __asm volatile("csrsi mstatus, 8" ::: "memory");
}

static void stop_timer(void) {
    TIMER(0x64u) = 0;
    TIMER(0x00u) = TIMER_BASE_CONFIG;
    TIMER(0x6cu) = 1;
    __asm volatile("fence iorw, iorw" ::: "memory");
}

static void stack_sample(void) {
    uint32_t *first = _stack_bottom;
    while (first < _stack_top && *first == UINT32_C(0xa5a5a5a5)) first++;
    uint32_t used = (uint32_t)((uintptr_t)_stack_top - (uintptr_t)first);
    if (used > board_stack_used) board_stack_used = used;
    board_stack_guard_ok = 1;
    for (uint32_t *p = _stack_guard_start; p < _stack_bottom; p++)
        if (*p != UINT32_C(0xdeadc0de)) board_stack_guard_ok = 0;
}

static void uart_bytes(const char *data, uint16_t length) {
    for (uint16_t i = 0; i < length; i++) {
        while (((UART_STATUS >> 16) & UINT32_C(0x3ff)) >= 127u) __asm volatile("nop");
        UART_FIFO = (uint8_t)data[i];
        board_uart_bytes++;
    }
}

_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
    (void)kind;
    irq_disable();
    stop_timer();
    GPIO_OUT_CLEAR = configured_outputs;
    board_failures++;
    if (!in_trap) {
        uart_bytes("Emela defect: ", 14);
        uart_bytes(message, length);
        uart_bytes("\n", 1);
        stack_sample();
    }
    for (;;) __asm volatile("wfi");
}

#define FAIL(text) emela_board_defect(EMELA_DEFECT_ASSERT, text, (uint16_t)(sizeof(text) - 1))

static void require_main(void) {
    uint32_t state;
    __asm volatile("csrr %0, mstatus" : "=r"(state));
    if (in_trap || !(state & 8u) || event_locked) FAIL("board operation requires main context");
}

static void uart_init(void) {
    /* Select the 40 MHz XTAL so the divider does not depend on the CPU/APB clock. */
    REG(0x600c0010) |= BIT(2) | BIT(24);
    REG(0x60000078) |= BIT(23);
    REG(0x600c0018) |= BIT(2);
    REG(0x600c0018) &= ~BIT(2);
    REG(0x60000078) = BIT(25) | BIT(24) | BIT(22) | (UINT32_C(3) << 20);
    REG(0x60000014) = UINT32_C(347) | (UINT32_C(3) << 20);
    REG(0x60000020) = UINT32_C(0x1c);
    /* UART0 TX/RX use the native IO_MUX functions on GPIO21/GPIO20. */
    IO_MUX(21) = UINT32_C(2) << 10;
    IO_MUX(20) = (UINT32_C(2) << 10) | BIT(9) | BIT(8);
    REG(0x60000080) &= ~BIT(30);
    REG(0x60000080) |= BIT(31);
    while (REG(0x60000080) & BIT(31)) {}
}

static void board_init(void) {
    /* Disable the ROM boot watchdogs before starting application scheduling. */
    REG(0x600080a8) = UINT32_C(0x50d83aa1);
    REG(0x60008090) &= ~(BIT(31) | BIT(12));
    REG(0x600080a8) = 0;
    REG(0x600080b0) = UINT32_C(0x8f1d312a);
    REG(0x600080ac) |= BIT(30);
    REG(0x600080b0) = 0;
    for (unsigned group = 0; group < 2; group++) {
        uintptr_t base = UINT32_C(0x6001f000) + group * UINT32_C(0x1000);
        REG(base + 0x64u) = UINT32_C(0x50d83aa1);
        REG(base + 0x48u) &= ~(BIT(31) | BIT(14));
        REG(base + 0x64u) = 0;
    }
    uart_init();
    REG(0x600c0010) |= BIT(29);
    REG(0x600c0018) &= ~BIT(29);
    MATRIX(0x104u) = 0;
    for (unsigned source = 0; source < 62; source++) MATRIX(source * 4u) = 0;
    TIMER(0x00u) = TIMER_BASE_CONFIG;
    TIMER(0x64u) = 0;
    TIMER(0x6cu) = 7;
    MATRIX(0x094u) = CPU_IRQ;
    MATRIX(0x108u) = 0;
    MATRIX(0x10cu) = UINT32_MAX;
    MATRIX(0x118u) = 1;
    MATRIX(0x194u) = 1;
    MATRIX(0x104u) = BIT(CPU_IRQ);
    __asm volatile("fence iorw, iorw\ncsrsi mstatus, 8" ::: "memory");
}

uint32_t emela_board_event_lock(void) {
    uint32_t previous = irq_disable();
    if (event_locked) FAIL("nested event lock");
    event_locked = 1;
    return previous;
}

void emela_board_event_unlock(uint32_t previous) {
    if (!event_locked) FAIL("invalid event unlock");
    event_locked = 0;
    irq_restore(previous);
}

void emela_board_console_print_line(const char *data, uint16_t length) {
    require_main();
    uart_bytes(data, length);
    uart_bytes("\n", 1);
}

void emela_board_console_print_error_line(const char *data, uint16_t length) {
    emela_board_console_print_line(data, length);
}

#ifdef BOARD_GPIO
static uint32_t pin_number(EmelaGpioPin pin) {
    /* GPIO11 supplies flash power; GPIO12..17 are reserved for the SPI flash. */
    if (pin.id < 0 || pin.id > 21 || (pin.id >= 11 && pin.id <= 17)) FAIL("unsupported GPIO pin");
    return (uint32_t)pin.id;
}

void emela_gpio_configure(EmelaGpioPin pin, EmelaGpioMode mode, EmelaGpioPull pull) {
    require_main();
    uint32_t number = pin_number(pin);
    if ((mode != EMELA_GPIO_MODE_INPUT && mode != EMELA_GPIO_MODE_OUTPUT_PUSH_PULL)
        || pull < EMELA_GPIO_PULL_NO_PULL || pull > EMELA_GPIO_PULL_DOWN)
        FAIL("unsupported GPIO mode or pull");
    uint32_t bit = BIT(number);
    GPIO_ENABLE_CLEAR = bit;
    GPIO_OUT_CLEAR = bit;
    GPIO_PIN(number) = 0;
    GPIO_OUT_SELECT(number) = UINT32_C(128) | BIT(9);
    IO_MUX(number) = (UINT32_C(1) << 12) | (UINT32_C(2) << 10) | BIT(9)
        | (pull == EMELA_GPIO_PULL_UP ? BIT(8) : 0)
        | (pull == EMELA_GPIO_PULL_DOWN ? BIT(7) : 0);
    configured_inputs |= bit;
    configured_outputs &= ~bit;
    if (mode == EMELA_GPIO_MODE_OUTPUT_PUSH_PULL) {
        configured_outputs |= bit;
        GPIO_ENABLE_SET = bit;
    }
}

EmelaGpioPinState emela_gpio_read(EmelaGpioPin pin) {
    require_main();
    uint32_t bit = BIT(pin_number(pin));
    if (!(configured_inputs & bit)) FAIL("GPIO input is not configured");
    return GPIO_IN & bit ? EMELA_GPIO_PIN_STATE_HIGH : EMELA_GPIO_PIN_STATE_LOW;
}

void emela_gpio_write(EmelaGpioPin pin, EmelaGpioPinState state) {
    require_main();
    uint32_t bit = BIT(pin_number(pin));
    if (!(configured_outputs & bit) || (state != EMELA_GPIO_PIN_STATE_LOW && state != EMELA_GPIO_PIN_STATE_HIGH))
        FAIL("GPIO output is not configured or state is invalid");
    if (state == EMELA_GPIO_PIN_STATE_HIGH) GPIO_OUT_SET = bit;
    else GPIO_OUT_CLEAR = bit;
}

void emela_gpio_toggle(EmelaGpioPin pin) {
    require_main();
    uint32_t bit = BIT(pin_number(pin));
    emela_gpio_write(pin, GPIO_OUT & bit ? EMELA_GPIO_PIN_STATE_LOW : EMELA_GPIO_PIN_STATE_HIGH);
}

void emela_gpio_enable_interrupt(EmelaGpioPin pin, EmelaGpioEdge edge) {
    (void)pin; (void)edge;
    FAIL("GPIO interrupts are not implemented by this board package");
}

void emela_gpio_disable_interrupt(EmelaGpioPin pin) {
    (void)pin;
    FAIL("GPIO interrupts are not implemented by this board package");
}
#endif

#ifdef BOARD_UART
int32_t emela_uart_configure(EmelaUartId id, EmelaUartConfig config) {
    require_main();
    if (id != EMELA_UART_ID_UART0 || config.baud_rate != 115200
        || config.data_bits != EMELA_UART_DATA_BITS_BITS8 || config.stop_bits != EMELA_UART_STOP_BITS_ONE
        || config.parity != EMELA_UART_PARITY_NO_PARITY || config.direction != EMELA_UART_DIRECTION_TRANSMIT
        || config.flow_control != EMELA_UART_FLOW_CONTROL_NO_FLOW_CONTROL
        || config.oversampling != EMELA_UART_OVERSAMPLING_BY16) return 1;
    uart_init();
    return 0;
}

int32_t emela_uart_transmit(EmelaUartId id, const uint16_t *data, uint16_t length, int64_t timeout) {
    require_main();
    (void)id; (void)data; (void)length; (void)timeout;
    return 2;
}

int32_t emela_uart_receive(EmelaUartId id, int32_t size, int64_t timeout,
                         uint16_t *out, uint16_t capacity, uint16_t *length) {
    require_main();
    (void)id; (void)size; (void)timeout; (void)out; (void)capacity;
    *length = 0;
    return 2;
}
#endif

#ifdef BOARD_CLOCK
static uint64_t timer_ticks(void) {
    TIMER(0x04u) = BIT(30);
    while (!(TIMER(0x04u) & BIT(29))) {}
    uint32_t hi = TIMER(0x40u);
    uint32_t lo = TIMER(0x44u);
    return ((uint64_t)hi << 32) | lo;
}

int64_t emela_board_clock_now(void) {
    require_main();
    uint32_t previous = irq_disable();
    uint64_t ticks = timer_ticks();
    irq_restore(previous);
    /* Monotonic milliseconds with a virtual Unix epoch, without a wall-clock RTC. */
    return (int64_t)(ticks / TICKS_PER_MS);
}

#ifdef EMELA_HAS_TASK_RUNTIME
void emela_board_clock_sleep(EmelaTask task, int32_t milliseconds) {
    require_main();
    if (milliseconds <= 0) {
        if (!emela_task_complete_unit(task)) FAIL("immediate clock completion rejected");
        return;
    }
    if (remaining_ms) FAIL("concurrent clock sleep exceeds board capacity");
    uint32_t previous = irq_disable();
    stop_timer();
    sleeping_task = task;
    remaining_ms = (uint32_t)milliseconds;
    uint64_t target = timer_ticks() + TICKS_PER_MS;
    TIMER(0x1cu) = (uint32_t)(target >> 32);
    TIMER(0x20u) = (uint32_t)target;
    TIMER(0x34u) = BIT(30) | TICKS_PER_MS;
    TIMER(0x50u) = 1;
    TIMER(0x64u) = 1;
    TIMER(0x00u) = TIMER_BASE_CONFIG | BIT(24);
    __asm volatile("fence iorw, iorw" ::: "memory");
    irq_restore(previous);
}
#endif
#endif

void board_trap(uint32_t cause, uint32_t pc, uintptr_t sp) {
    in_trap = 1;
    board_trap_cause = cause;
    board_trap_pc = pc;
    uint32_t used = (uint32_t)((uintptr_t)_stack_top - sp);
    if (used > board_stack_used) board_stack_used = used;
#if defined(BOARD_CLOCK) && defined(EMELA_HAS_TASK_RUNTIME)
    if ((cause & UINT32_C(0x80000fff)) != (UINT32_C(0x80000000) | CPU_IRQ)
        || !(TIMER(0x70u) & 1u) || !remaining_ms) FAIL("unexpected CPU trap");
    TIMER(0x6cu) = 1;
    board_ticks++;
    if (--remaining_ms == 0) {
        stop_timer();
        if (emela_task_complete_unit(sleeping_task)) board_notifications++;
        /* A canceled task may leave one stale timer completion, rejected by generation. */
        else board_rejected_notifications++;
    }
#else
    FAIL("unexpected CPU trap");
#endif
    __asm volatile("fence iorw, iorw" ::: "memory");
    in_trap = 0;
}

void board_main(void) {
    stack_sample();
    board_init();
    int32_t result = -1;
    EmelaFiber app = board_main_start(&result);
    while (!emela_fiber_done(app)) {
        uint32_t observed = board_ticks;
        bool ran = false;
        while (emela_scheduler_run(1)) ran = true;
        if (ran) stack_sample();
        if (emela_fiber_done(app)) break;
        uint32_t previous = irq_disable();
        if (board_ticks == observed) __asm volatile("fence iorw, iorw\nwfi" ::: "memory");
        irq_restore(previous);
    }
    stop_timer();
    GPIO_OUT_CLEAR = configured_outputs;
    emela_fiber_release(app);
    board_app_result = result;
    stack_sample();
    board_app_completed = 1;
    irq_disable();
    for (;;) __asm volatile("wfi");
}

/* ABI 2 lifecycle for consumers of the common entry wrapper. */
void emela_board_idle(void) {
    uint32_t previous = irq_disable();
    if (!emela_scheduler_ready()) __asm volatile("fence iorw, iorw\nwfi" ::: "memory");
    irq_restore(previous);
}
void emela_board_debug(const char *bytes, uint16_t length) { uart_bytes(bytes, length); }
void emela_platform_start(void (*program)(void)) {
    board_init();
    program();
    for (;;) emela_board_idle();
}
