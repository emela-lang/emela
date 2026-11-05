// SPDX-License-Identifier: Apache-2.0
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    // Observe endpoint wires independently of GPIO MMIO readback and firmware globals.
    [GPIO(NumberOfInputs = 2)]
    public class STM32F4ActuatorObserver : IGPIOReceiver
    {
        public void Reset()
        {
            State = 0;
            TransitionCount = 0;
        }

        public void OnGPIO(int number, bool value)
        {
            if(number < 0 || number >= 2)
            {
                this.Log(LogLevel.Warning, "Unsupported observer input {0}", number);
                return;
            }
            var mask = 1u << number;
            if(((State & mask) != 0) == value)
            {
                return;
            }
            State = value ? State | mask : State & ~mask;
            TransitionCount++;
            this.Log(LogLevel.Info, "EMELA_F4_GPIO pin={0} level={1}", number, value ? 1 : 0);
        }

        public uint State { get; private set; }
        public ulong TransitionCount { get; private set; }
    }
}
