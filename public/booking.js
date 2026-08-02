(() => {
  const modal = document.getElementById('booking-modal');
  if (!modal) return;

  const serviceSelect = document.getElementById('booking-service');
  const availability = document.getElementById('booking-availability');
  const daysHost = document.getElementById('booking-days');
  const slotsHost = document.getElementById('booking-slots');
  const monthTitle = document.getElementById('booking-month-title');
  const prevMonth = document.getElementById('booking-prev-month');
  const nextMonth = document.getElementById('booking-next-month');
  const timeTitle = document.getElementById('booking-time-title');
  const form = document.getElementById('booking-form');
  const alertBox = document.getElementById('booking-alert');
  let services = [];
  let availableDays = [];
  let rangeStart = '';
  let rangeEnd = '';
  let calendarMonth = null;
  let selectedDate = '';
  let selectedStart = '';
  let returnFocus = null;
  let formStartedAt = Date.now();

  async function request(url, options = {}) {
    const response = await fetch(url, { ...options, headers: { 'Content-Type': 'application/json', ...(options.headers || {}) } });
    const body = await response.json();
    if (!response.ok) throw new Error(body?.error?.message || 'İşlem tamamlanamadı.');
    return body;
  }

  async function loadServices() {
    if (services.length) return;
    const result = await request('/api/services');
    services = result.services;
    services.forEach(service => {
      const option = document.createElement('option');
      option.value = service.id;
      option.textContent = service.name;
      serviceSelect.appendChild(option);
    });
  }

  async function open(serviceId = '') {
    returnFocus = document.activeElement;
    modal.classList.add('active');
    modal.setAttribute('aria-hidden', 'false');
    document.body.style.overflow = 'hidden';
    modal.querySelector('[data-booking-close]').focus();
    formStartedAt = Date.now();
    resetAfterSuccess();
    clearAlert();
    try {
      await loadServices();
      serviceSelect.value = serviceId;
      if (serviceId) await loadAvailability();
      else resetAvailability();
    } catch (error) {
      showAlert(error.message, 'error');
    }
  }

  function close() {
    modal.classList.remove('active');
    modal.setAttribute('aria-hidden', 'true');
    document.body.style.overflow = '';
    if (returnFocus) returnFocus.focus();
  }

  function showAlert(message, type) {
    alertBox.textContent = message;
    alertBox.className = `booking-alert show ${type}`;
    alertBox.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
  }

  function clearAlert() {
    alertBox.className = 'booking-alert';
    alertBox.textContent = '';
  }

  function resetAvailability() {
    availability.hidden = true;
    form.hidden = true;
    selectedDate = '';
    selectedStart = '';
  }

  async function loadAvailability() {
    clearAlert();
    selectedDate = '';
    selectedStart = '';
    form.hidden = true;
    if (!serviceSelect.value) return resetAvailability();
    availability.hidden = false;
    daysHost.innerHTML = '<div class="booking-loading">Takvim hazırlanıyor…</div>';
    slotsHost.innerHTML = '';
    timeTitle.hidden = true;
    try {
      const result = await request(`/api/availability?service=${encodeURIComponent(serviceSelect.value)}`);
      availableDays = result.days || [];
      rangeStart = result.rangeStart;
      rangeEnd = result.rangeEnd;
      calendarMonth = new Date(`${rangeStart.slice(0, 7)}-01T00:00:00Z`);
      renderCalendar();
      if (!availableDays.length) showAlert('Bu hizmet için yayınlanmış uygun bir tarih bulunmuyor.', 'info');
    } catch (error) {
      showAlert(error.message, 'error');
    }
  }

  function renderCalendar() {
    const year = calendarMonth.getUTCFullYear();
    const month = calendarMonth.getUTCMonth();
    monthTitle.textContent = new Intl.DateTimeFormat('tr-TR', { month: 'long', year: 'numeric', timeZone: 'UTC' }).format(calendarMonth);
    daysHost.innerHTML = '';
    const firstWeekday = (new Date(Date.UTC(year, month, 1)).getUTCDay() + 6) % 7;
    const daysInMonth = new Date(Date.UTC(year, month + 1, 0)).getUTCDate();
    for (let blank = 0; blank < firstWeekday; blank++) daysHost.appendChild(document.createElement('span'));
    for (let day = 1; day <= daysInMonth; day++) {
      const dateKey = `${year}-${String(month + 1).padStart(2, '0')}-${String(day).padStart(2, '0')}`;
      const dayData = availableDays.find(item => item.date === dateKey);
      const button = document.createElement('button');
      button.type = 'button';
      button.className = `booking-day${selectedDate === dateKey ? ' active' : ''}`;
      button.textContent = day;
      button.disabled = !dayData || dateKey < rangeStart || dateKey > rangeEnd;
      button.setAttribute('aria-label', dayData ? `${dayData.label}, uygun saatleri göster` : `${day} tarihi müsait değil`);
      if (dayData) button.addEventListener('click', () => selectDate(dayData));
      daysHost.appendChild(button);
    }
    const previousEnd = new Date(Date.UTC(year, month, 0)).toISOString().slice(0, 10);
    const nextStart = new Date(Date.UTC(year, month + 1, 1)).toISOString().slice(0, 10);
    prevMonth.disabled = previousEnd < rangeStart;
    nextMonth.disabled = nextStart > rangeEnd;
  }

  function selectDate(dayData) {
    selectedDate = dayData.date;
    selectedStart = '';
    form.hidden = true;
    renderCalendar();
    timeTitle.hidden = false;
    timeTitle.textContent = `${dayData.label} için uygun saatler`;
    slotsHost.innerHTML = '';
    dayData.slots.forEach(slot => {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'booking-slot';
      button.textContent = slot.label;
      button.addEventListener('click', () => selectSlot(dayData, slot));
      slotsHost.appendChild(button);
    });
  }

  function selectSlot(dayData, slot) {
    selectedStart = slot.start;
    slotsHost.querySelectorAll('.booking-slot').forEach(button => button.classList.toggle('active', button.textContent === slot.label));
    const service = services.find(item => item.id === serviceSelect.value);
    document.getElementById('booking-summary').textContent = `${service.name} — ${dayData.label} ${slot.label}`;
    form.hidden = false;
    form.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }

  prevMonth.addEventListener('click', () => {
    calendarMonth = new Date(Date.UTC(calendarMonth.getUTCFullYear(), calendarMonth.getUTCMonth() - 1, 1));
    renderCalendar();
  });
  nextMonth.addEventListener('click', () => {
    calendarMonth = new Date(Date.UTC(calendarMonth.getUTCFullYear(), calendarMonth.getUTCMonth() + 1, 1));
    renderCalendar();
  });
  serviceSelect.addEventListener('change', loadAvailability);
  document.getElementById('booking-note').addEventListener('input', event => {
    document.getElementById('booking-note-count').textContent = event.target.value.length;
  });

  form.addEventListener('submit', async event => {
    event.preventDefault();
    clearAlert();
    if (!form.reportValidity() || !selectedStart) return;
    const button = form.querySelector('button[type="submit"]');
    button.disabled = true;
    button.textContent = 'Randevu gönderiliyor…';
    try {
      const result = await request('/api/appointments', {
        method: 'POST',
        body: JSON.stringify({
          serviceId: serviceSelect.value,
          start: selectedStart,
          name: document.getElementById('booking-name').value,
          email: document.getElementById('booking-email').value,
          phone: document.getElementById('booking-phone').value,
          note: document.getElementById('booking-note').value,
          website: document.getElementById('booking-website').value,
          consent: document.getElementById('booking-consent').checked,
          startedAt: formStartedAt
        })
      });
      document.getElementById('booking-service-field').hidden = true;
      availability.hidden = true;
      form.hidden = true;
      document.getElementById('booking-request-id').textContent = `Randevu numarası: ${result.appointment.id}`;
      document.getElementById('booking-success').hidden = false;
    } catch (error) {
      await loadAvailability();
      showAlert(error.message, 'error');
      button.disabled = false;
      button.textContent = 'Randevu seçimini gönder';
    }
  });

  document.querySelectorAll('.booking-trigger').forEach(button => button.addEventListener('click', () => open(button.dataset.service || '')));
  modal.querySelectorAll('[data-booking-close]').forEach(button => button.addEventListener('click', close));
  document.addEventListener('keydown', event => {
    if (event.key === 'Escape' && modal.classList.contains('active')) close();
  });

  function resetAfterSuccess() {
    const success = document.getElementById('booking-success');
    if (success.hidden) return;
    success.hidden = true;
    document.getElementById('booking-service-field').hidden = false;
    serviceSelect.value = '';
    resetAvailability();
    form.reset();
    document.getElementById('booking-note-count').textContent = '0';
    form.querySelector('button[type="submit"]').disabled = false;
    form.querySelector('button[type="submit"]').textContent = 'Randevu seçimini gönder';
  }
})();
