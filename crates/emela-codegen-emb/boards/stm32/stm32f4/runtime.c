/* SPDX-License-Identifier: Apache-2.0 */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "board_config.h"
#include "emela_embedded_exports.h"
#include "emela_board.h"
#include "validation_stack.h"
#if __has_include("emela_gpio.h")
#include "emela_gpio.h"
#define BOARD_HAS_GPIO 1
#endif
#if __has_include("emela_uart.h")
#include "emela_uart.h"
#define BOARD_HAS_UART 1
#endif

#define REG(address) (*(volatile uint32_t *)(uintptr_t)(address))
#define GPIO_REG(port, offset) REG(UINT32_C(0x40020000) + (port) * UINT32_C(0x400) + (offset))
#define GPIO_MODER(port) GPIO_REG(port, UINT32_C(0x00))
#define GPIO_OTYPER(port) GPIO_REG(port, UINT32_C(0x04))
#define GPIO_PUPDR(port) GPIO_REG(port, UINT32_C(0x0c))
#define GPIO_IDR(port) GPIO_REG(port, UINT32_C(0x10))
#define GPIO_ODR(port) GPIO_REG(port, UINT32_C(0x14))
#define GPIO_BSRR(port) GPIO_REG(port, UINT32_C(0x18))
#define GPIO_AFR(port, pin) GPIO_REG(port, UINT32_C(0x20) + ((pin) / 8) * 4)
#define RCC_AHB1ENR REG(UINT32_C(0x40023830))
#define RCC_APB1ENR REG(UINT32_C(0x40023840))
#define RCC_APB2ENR REG(UINT32_C(0x40023844))
#define UART_REG(offset) REG(EMELA_CONSOLE_UART_BASE + (offset))
#define UART_SR UART_REG(UINT32_C(0x00))
#define UART_DR UART_REG(UINT32_C(0x04))
#define UART_BRR UART_REG(UINT32_C(0x08))
#define UART_CR1 UART_REG(UINT32_C(0x0c))
#define UART_CR2 UART_REG(UINT32_C(0x10))
#define UART_CR3 UART_REG(UINT32_C(0x14))
#define SYSTICK_CTRL REG(UINT32_C(0xe000e010))
#define SYSTICK_LOAD REG(UINT32_C(0xe000e014))
#define SYSTICK_VALUE REG(UINT32_C(0xe000e018))

volatile uint32_t board_app_completed;
volatile int32_t board_app_result;
volatile uint32_t board_app_failures;
volatile uint32_t board_ticks;
volatile uint32_t board_notifications;
volatile uint32_t board_rejected_notifications;
volatile uint32_t board_uart_bytes;
/* The reset path must copy .data and clear .bss without debugger assistance. */
volatile uint32_t board_boot_data = UINT32_C(0x454d4244);
volatile uint32_t board_boot_bss;
volatile uint32_t board_boot_initialized;

static unsigned event_locked;
static uint32_t output_pins[EMELA_GPIO_PORT_COUNT];
static bool uart_ready;
static volatile uint64_t elapsed_ms;
#ifdef EMELA_HAS_TASK_RUNTIME
static volatile uint32_t remaining_ms;
static volatile EmelaTask sleeping_task;
#endif

void _init(void) {}
void _fini(void) {}

static uint32_t interrupt_state(void) {
    uint32_t value;
    __asm volatile("mrs %0, primask" : "=r"(value));
    return value;
}

static uint32_t exception_number(void) {
    uint32_t value;
    __asm volatile("mrs %0, ipsr" : "=r"(value));
    return value;
}

static void uart_bytes(const char *bytes, uint16_t length) {
    for (uint16_t i = 0; i < length; ++i) {
        while ((UART_SR & (UINT32_C(1) << 7)) == 0) __asm volatile("nop");
        UART_DR = (uint8_t)bytes[i];
        board_uart_bytes++;
    }
}

_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
    (void)kind;
    SYSTICK_CTRL = 0;
    for (uint32_t port = 0; port < EMELA_GPIO_PORT_COUNT; ++port) {
        if (output_pins[port] != 0) GPIO_BSRR(port) = output_pins[port] << 16;
    }
    board_app_failures++;
    if (exception_number() == 0) {
        if (uart_ready) {
            uart_bytes(message, length);
            uart_bytes("\n", 1);
        }
        emela_validation_stack_sample();
    }
    __asm volatile("cpsid i" ::: "memory");
    for (;;) __asm volatile("wfi");
}

#define FAIL(message) emela_board_defect(EMELA_DEFECT_ASSERT, message, (uint16_t)(sizeof(message) - 1))

static void main_context(void) {
    if (exception_number() != 0 || interrupt_state() != 0 || event_locked != 0)
        FAIL("board operation requires main context");
}

static void init_uart_pin(uint32_t id) {
    uint32_t port = id / 16;
    uint32_t pin = id % 16;
    uint32_t shift = pin * 2;
    uint32_t af_shift = (pin % 8) * 4;
    RCC_AHB1ENR |= UINT32_C(1) << port;
    (void)RCC_AHB1ENR;
    GPIO_OTYPER(port) &= ~(UINT32_C(1) << pin);
    GPIO_PUPDR(port) &= ~(UINT32_C(3) << shift);
    GPIO_AFR(port, pin) = (GPIO_AFR(port, pin) & ~(UINT32_C(15) << af_shift))
        | ((uint32_t)EMELA_CONSOLE_UART_AF << af_shift);
    GPIO_MODER(port) = (GPIO_MODER(port) & ~(UINT32_C(3) << shift)) | (UINT32_C(2) << shift);
}

static void init_uart(void) {
    init_uart_pin(EMELA_CONSOLE_TX_PIN);
    init_uart_pin(EMELA_CONSOLE_RX_PIN);
#if EMELA_CONSOLE_UART_APB == 1
    RCC_APB1ENR |= UINT32_C(1) << EMELA_CONSOLE_UART_RCC_BIT;
    (void)RCC_APB1ENR;
#elif EMELA_CONSOLE_UART_APB == 2
    RCC_APB2ENR |= UINT32_C(1) << EMELA_CONSOLE_UART_RCC_BIT;
    (void)RCC_APB2ENR;
#else
#error "Console UART must be attached to APB1 or APB2"
#endif
    /* All selected profiles retain reset HSI/APB clocks at 16 MHz. */
    UART_CR1 = 0;
    UART_CR2 = 0;
    UART_CR3 = 0;
    UART_BRR = 139;
    UART_CR1 = (UINT32_C(1) << 13) | (UINT32_C(1) << 3) | (UINT32_C(1) << 2);
    uart_ready = true;
}

#if BOARD_HAS_GPIO
static const uint16_t gpio_pin_masks[EMELA_GPIO_PORT_COUNT] = EMELA_GPIO_PIN_MASKS;

static uint32_t pin_port(EmelaGpioPin pin) {
    if (pin.id < 0 || (uint32_t)pin.id >= EMELA_GPIO_PORT_COUNT * 16)
        FAIL("GPIO logical pin is outside this MCU package");
    uint32_t port = (uint32_t)pin.id / 16;
    uint32_t bit = UINT32_C(1) << ((uint32_t)pin.id % 16);
    if (!(gpio_pin_masks[port] & bit)) FAIL("GPIO pin is absent from this MCU package");
    if (pin.id == EMELA_CONSOLE_TX_PIN || pin.id == EMELA_CONSOLE_RX_PIN)
        FAIL("GPIO pin is reserved for the board Console UART");
    return port;
}

void emela_gpio_configure(EmelaGpioPin pin, EmelaGpioMode mode, EmelaGpioPull pull) {
    main_context();
    uint32_t port = pin_port(pin);
    uint32_t bit = UINT32_C(1) << ((uint32_t)pin.id % 16);
    if (mode < EMELA_GPIO_MODE_INPUT || mode > EMELA_GPIO_MODE_ANALOG
        || pull < EMELA_GPIO_PULL_NO_PULL || pull > EMELA_GPIO_PULL_DOWN)
        FAIL("unsupported GPIO configuration");
    uint32_t shift = ((uint32_t)pin.id % 16) * 2;
    uint32_t hardware_mode = mode == EMELA_GPIO_MODE_ANALOG ? 3
        : mode == EMELA_GPIO_MODE_INPUT ? 0 : 1;
    RCC_AHB1ENR |= UINT32_C(1) << port;
    (void)RCC_AHB1ENR;
    GPIO_BSRR(port) = bit << 16;
    GPIO_OTYPER(port) = mode == EMELA_GPIO_MODE_OUTPUT_OPEN_DRAIN
        ? GPIO_OTYPER(port) | bit : GPIO_OTYPER(port) & ~bit;
    GPIO_PUPDR(port) = (GPIO_PUPDR(port) & ~(UINT32_C(3) << shift)) | ((uint32_t)pull << shift);
    GPIO_MODER(port) = (GPIO_MODER(port) & ~(UINT32_C(3) << shift)) | (hardware_mode << shift);
    output_pins[port] = hardware_mode == 1 ? output_pins[port] | bit : output_pins[port] & ~bit;
}

void emela_gpio_write(EmelaGpioPin pin, EmelaGpioPinState state) {
    main_context();
    uint32_t port = pin_port(pin);
    uint32_t bit = UINT32_C(1) << ((uint32_t)pin.id % 16);
    if (!(output_pins[port] & bit) || (state != EMELA_GPIO_PIN_STATE_LOW && state != EMELA_GPIO_PIN_STATE_HIGH))
        FAIL("GPIO write requires a configured output");
    GPIO_BSRR(port) = state == EMELA_GPIO_PIN_STATE_HIGH ? bit : bit << 16;
}

EmelaGpioPinState emela_gpio_read(EmelaGpioPin pin) {
    main_context();
    uint32_t port = pin_port(pin);
    uint32_t bit = UINT32_C(1) << ((uint32_t)pin.id % 16);
    return (GPIO_IDR(port) & bit) ? EMELA_GPIO_PIN_STATE_HIGH : EMELA_GPIO_PIN_STATE_LOW;
}

void emela_gpio_toggle(EmelaGpioPin pin) {
    main_context();
    uint32_t port = pin_port(pin);
    uint32_t bit = UINT32_C(1) << ((uint32_t)pin.id % 16);
    emela_gpio_write(pin, (GPIO_ODR(port) & bit) ? EMELA_GPIO_PIN_STATE_LOW : EMELA_GPIO_PIN_STATE_HIGH);
}

void emela_gpio_enable_interrupt(EmelaGpioPin pin, EmelaGpioEdge edge) {
    (void)pin;
    (void)edge;
    FAIL("GPIO interrupts require a project-specific driver");
}

void emela_gpio_disable_interrupt(EmelaGpioPin pin) {
    (void)pin;
    FAIL("GPIO interrupts require a project-specific driver");
}
#endif

#if BOARD_HAS_UART
int32_t emela_uart_configure(EmelaUartId id, EmelaUartConfig config) {
    main_context();
    if (id != EMELA_CONSOLE_UART_ID || config.baud_rate != 115200
        || config.data_bits != EMELA_UART_DATA_BITS_BITS8
        || config.stop_bits != EMELA_UART_STOP_BITS_ONE
        || config.parity != EMELA_UART_PARITY_NO_PARITY
        || config.direction != EMELA_UART_DIRECTION_TRANSMIT
        || config.flow_control != EMELA_UART_FLOW_CONTROL_NO_FLOW_CONTROL
        || config.oversampling != EMELA_UART_OVERSAMPLING_BY16) return 1;
    init_uart();
    return 0;
}

int32_t emela_uart_transmit(EmelaUartId id, const uint16_t *data, uint16_t length, int64_t timeout_ms) {
    main_context();
    (void)id; (void)data; (void)length; (void)timeout_ms;
    return 2;
}

int32_t emela_uart_receive(EmelaUartId id, int32_t size, int64_t timeout_ms,
                           uint16_t *out, uint16_t capacity, uint16_t *length) {
    main_context();
    (void)id; (void)size; (void)timeout_ms; (void)out; (void)capacity;
    *length = 0;
    return 2;
}
#endif

void emela_board_console_print_line(const char *data, uint16_t length) {
    main_context();
    uart_bytes(data, length);
    uart_bytes("\n", 1);
}

void emela_board_console_print_error_line(const char *data, uint16_t length) {
    emela_board_console_print_line(data, length);
}

uint32_t emela_board_event_lock(void) {
    uint32_t previous = interrupt_state();
    __asm volatile("cpsid i" ::: "memory");
    if (event_locked != 0) FAIL("nested task event lock");
    event_locked = 1;
    return previous;
}

void emela_board_event_unlock(uint32_t previous) {
    if (event_locked != 1 || interrupt_state() == 0) FAIL("invalid task event unlock");
    event_locked = 0;
    __asm volatile("msr primask, %0" :: "r"(previous) : "memory");
}

int64_t emela_board_clock_now(void) {
    main_context();
    uint32_t previous = interrupt_state();
    __asm volatile("cpsid i" ::: "memory");
    uint64_t value = elapsed_ms;
    __asm volatile("msr primask, %0" :: "r"(previous) : "memory");
    /* The epoch is boot-time zero; this package does not supply a wall-clock RTC. */
    return (int64_t)value;
}

#ifdef EMELA_HAS_TASK_RUNTIME
void emela_board_clock_sleep(EmelaTask task, int32_t milliseconds) {
    main_context();
    if (milliseconds <= 0) {
        if (!emela_task_complete_unit(task)) FAIL("immediate timer completion rejected");
        return;
    }
    if (remaining_ms != 0) FAIL("only one concurrent Clock.sleep is supported");
    uint32_t previous = interrupt_state();
    __asm volatile("cpsid i" ::: "memory");
    sleeping_task = task;
    remaining_ms = (uint32_t)milliseconds;
    __asm volatile("msr primask, %0" :: "r"(previous) : "memory");
}
#endif

void SysTick_Handler(void) {
    emela_validation_stack_record_sp();
    board_ticks++;
    elapsed_ms++;
#ifdef EMELA_HAS_TASK_RUNTIME
    if (remaining_ms != 0 && --remaining_ms == 0) {
        /* Cancellation can invalidate a task before the physical timer expires. */
        if (emela_task_complete_unit(sleeping_task)) board_notifications++;
        else board_rejected_notifications++;
    }
#endif
}

int main(void) {
    if (board_boot_data != UINT32_C(0x454d4244) || board_boot_bss != 0)
        FAIL("reset data/bss initialization failed");
    board_boot_bss = UINT32_C(0x4253534f);
    board_boot_initialized = 1;
    init_uart();
    SYSTICK_LOAD = 16000 - 1;
    SYSTICK_VALUE = 0;
    SYSTICK_CTRL = 7;
    int32_t result = 0;
    EmelaFiber app = board_main_start(&result);
    while (!emela_fiber_done(app)) {
        uint32_t observed_tick = board_ticks;
        bool did_work = false;
        while (emela_scheduler_run(1)) did_work = true;
        if (did_work) emela_validation_stack_sample();
        if (emela_fiber_done(app)) break;
        /* Masked WFI wakes on pending interrupts and closes the empty-queue race. */
        uint32_t previous = interrupt_state();
        __asm volatile("cpsid i" ::: "memory");
        if (board_ticks == observed_tick) __asm volatile("dsb\nwfi" ::: "memory");
        __asm volatile("msr primask, %0" :: "r"(previous) : "memory");
    }
    SYSTICK_CTRL = 0;
    emela_fiber_release(app);
    board_app_result = result;
    emela_validation_stack_sample();
    board_app_completed = 1;
    for (;;) __asm volatile("wfi");
}
